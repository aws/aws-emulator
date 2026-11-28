// SPDX-License-Identifier: CC0-1.0

//! The CONCURRENCY CONTROL of a database whose engine resolves two writers
//! OPTIMISTICALLY: every transaction runs on the database as it stood when the
//! transaction opened, and a commit whose rows another transaction has changed
//! since is refused instead of a row being held against the second writer.
//!
//! A database engine has to say what happens when two sessions write the same
//! row at the same time, and the two answers differ in what a CLIENT sees:
//!
//! * a PESSIMISTIC engine holds the row — the second writer's statement waits
//!   for the first transaction to end, and then both commits stand;
//! * an OPTIMISTIC engine holds nothing — the second writer's statement answers
//!   at once, and the conflict is settled at COMMIT, where exactly one of the
//!   two is refused `serialization_failure` and leaves no effect at all.
//!
//! Which of the two an endpoint serves is the OWNING SERVICE's fact about its
//! product ([`super::ServedDatabase::concurrency`]), and this module is the
//! optimistic one. What it is built from is the engine's own account of itself,
//! never a reading of a statement's text:
//!
//! * the SNAPSHOT a transaction runs on is a page-for-page copy of the
//!   database, made by the engine's own online backup when the transaction opens
//!   ([`super::sql_engine::DatabaseEngine::copy`]). So what the transaction
//!   writes is in its own copy and nowhere else until it commits, what another
//!   session commits meanwhile does not move under it (the repeatable read every
//!   statement of it sees), and a transaction that goes away — a `ROLLBACK`, a
//!   session that ended — leaves the database exactly as it was, because dropping
//!   the copy is the whole of undoing it;
//! * WHICH ROWS it changed is the engine's update hook
//!   ([`super::sql_engine::StatementWrites`]), which reports every row an
//!   `INSERT`, `UPDATE` or `DELETE` touched, the rows a cascade or a trigger took
//!   with them included;
//! * a COMMIT carries over what the transaction's own copy HOLDS — for every row
//!   it changed, the value that copy holds for it, and the SCHEMA as the ENGINE's
//!   own catalog declares it — inside ONE engine transaction, so either all of it
//!   is there or none is. NO statement of the client's is kept and none is ever
//!   re-run: a read-modify-write commits the value the client saw inside the
//!   transaction.
//!
//! The conflict decision is between WRITERS, which is what an optimistic engine
//! settles: a transaction is refused when a row it changed, and that the database
//! already held when its snapshot was taken, has been changed by a transaction
//! that committed since. A row the transaction CREATED is not compared by row id
//! — the engine assigns one per database, so two transactions' new rows can be
//! given the same id without being the same row — and two such rows that really
//! are the same row meet the database's own key when the commit is applied, which
//! is refused as the same conflict. A transaction that READ rows it did not write
//! is not refused for them (the case
//! `a_commit_carries_the_value_the_transaction_computed_on_its_snapshot` pins
//! both halves of that).
//!
//! What it costs, stated rather than hidden: an open transaction holds a copy of
//! the database. Every part of it is BOUNDED: the objects one transaction may
//! declare or drop ([`MAX_TRANSACTION_DECLARATIONS`]), the rows it may have
//! changed ([`MAX_TRANSACTION_ROWS`]), the commits and the rows this module
//! remembers ([`REMEMBERED_COMMITS`], [`MAX_REMEMBERED_ROWS`]), the rows and the
//! bytes one statement's writes are recorded as
//! ([`super::sql_engine::MAX_RECORDED_ROWS`],
//! [`super::sql_engine::MAX_RECORDED_BYTES`]) and the engine's own ceiling on the
//! pages one database may hold. Past any of them the statement is refused rather
//! than served: a transaction that outgrew its bounds can no longer commit, and a
//! transaction older than the commits still remembered is refused as a conflict
//! rather than committed on an answer this module no longer holds.
//!
//! A change the engine keeps NO account of is refused for the same reason, rather
//! than committed as the part of it that was accounted for: a relation declared
//! WITHOUT ROWID, whose changes an update hook does not report; a statement that
//! declares a relation and FILLS it in one go, whose rows the engine writes
//! without reporting them; and a change to the DEFINITION of a relation the
//! database already held, which the catalog records as the definition it has now
//! and not as the change that got it there.
//!
//! Services still carrying their own copy of this reading: this module postdates
//! the Amazon RDS engine endpoint, whose
//! `src/state/data_plane/session_transactions.rs` holds a snapshot-per-session
//! transaction of its own, built on the same two engine facilities (the online
//! backup and the update hook). That copy is to be MIGRATED onto this module, as
//! the rest of that data plane is ([`super`]'s own header); it is left in place
//! only because a change does not own that service's issues, and because it
//! carries a great deal more than this module's surface — four isolation levels,
//! row locks and their `NOWAIT`/`SKIP LOCKED` forms, deadlock detection, a
//! cascading foreign-key action's rows, and a schema delta carried per commit —
//! none of which a PESSIMISTIC engine's rules need from here. Nothing new writes
//! a third copy: a service whose product resolves writers optimistically states
//! [`super::Concurrency::Optimistic`] and is served by this module.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use super::sql_engine::{
    Cell, ChangedRow, DatabaseEngine, EngineError, EngineErrorKind, EngineView, ResultSet,
};

/// How many objects one transaction may declare or drop before it commits.
///
/// CHOSEN for the same reason the row bound below is: the schema a commit carries
/// over is held while the transaction is open, and a transaction past this is
/// refused rather than growing without bound. The catalog itself is bounded by
/// the engine's own ceiling on the pages one database may hold.
pub const MAX_TRANSACTION_DECLARATIONS: usize = 2_000;

/// How many committed transactions' write sets are remembered, which is what a
/// still-open transaction is compared against.
///
/// CHOSEN: far more than the handful of transactions that overlap in an emulated
/// workload. A transaction whose snapshot is older than the oldest write set
/// still here cannot be shown to be free of conflicts, so it is refused as a
/// conflict — the answer that never lets an unchecked write stand.
pub const REMEMBERED_COMMITS: usize = 1_024;

/// How many changed ROWS those remembered write sets hold together.
///
/// CHOSEN for the same reason the count is, and beside it rather than instead of
/// it: one commit's write set is as large as the rows that commit changed, so a
/// count of commits alone would not bound the memory this register holds. The
/// oldest write sets are forgotten until both bounds hold, and a transaction
/// older than the oldest one still here is then refused as a conflict.
pub const MAX_REMEMBERED_ROWS: usize = 200_000;

/// How many changed ROWS one open transaction may hold.
///
/// CHOSEN for the same reason: the rows a transaction has changed are what its
/// commit is judged on, and they are held until it ends. A transaction that
/// outgrows this is refused and can no longer commit
/// ([`OpenTransaction::outgrew_its_bounds`]), so nothing it wrote is ever
/// carried over on a partial account of it.
pub const MAX_TRANSACTION_ROWS: usize = 200_000;

/// The sentence this module reports a refused commit with, which the owning
/// service's dialect words its own way ([`super::SqlDialect::refusal`]).
const CONFLICT_MESSAGE: &str = "could not serialize access due to concurrent update";

/// The optimistic concurrency control of ONE database: the commits it has
/// applied, in order, and the engine they were applied to.
///
/// Shared by every session on the endpoint, because what two writers conflict
/// over is the one database behind it.
#[derive(Debug)]
pub struct OptimisticWriters {
    /// The shared database: what a transaction's snapshot is copied from and
    /// what its commit is applied to.
    committed: Arc<DatabaseEngine>,
    /// The commits applied so far.
    applied: Mutex<AppliedCommits>,
}

/// The commits a still-open transaction may conflict with: a counter of the
/// commits applied, and the rows each of the most recent ones changed.
#[derive(Debug, Default)]
struct AppliedCommits {
    /// How many commits have been applied, which is the version a snapshot is
    /// taken at and the version a commit moves the database to.
    version: u64,
    /// The rows each remembered commit changed, newest last.
    changes: VecDeque<(u64, BTreeSet<ChangedRow>)>,
    /// How many rows those write sets hold together.
    remembered_rows: usize,
}

impl AppliedCommits {
    /// Remember that the commit which moved the database to `version` changed
    /// `rows`, forgetting the oldest write sets until both bounds hold.
    fn remember(&mut self, version: u64, rows: BTreeSet<ChangedRow>) {
        self.remembered_rows = self.remembered_rows.saturating_add(rows.len());
        self.changes.push_back((version, rows));
        while self.changes.len() > REMEMBERED_COMMITS
            || self.remembered_rows > MAX_REMEMBERED_ROWS
        {
            let Some((_version, forgotten)) = self.changes.pop_front() else {
                break;
            };
            self.remembered_rows = self.remembered_rows.saturating_sub(forgotten.len());
        }
    }

    /// Whether a transaction whose snapshot was taken at `at` wrote a row some
    /// commit since has changed — or is older than the commits still remembered,
    /// which cannot be shown to be free of conflicts.
    fn conflicts_with(&self, at: u64, rows: &BTreeSet<ChangedRow>) -> bool {
        if self.version > at && self.oldest_remembered() > at {
            return true;
        }
        self.changes
            .iter()
            .filter(|(version, _)| *version > at)
            .any(|(_, changed)| changed.intersection(rows).next().is_some())
    }

    /// The oldest version whose changed rows are still remembered.
    fn oldest_remembered(&self) -> u64 {
        self.changes
            .front()
            .map_or(self.version, |(version, _)| version.saturating_sub(1))
    }
}

impl OptimisticWriters {
    /// The optimistic concurrency control of the database `committed`.
    #[must_use]
    pub fn on(committed: Arc<DatabaseEngine>) -> Arc<Self> {
        Arc::new(Self {
            committed,
            applied: Mutex::new(AppliedCommits::default()),
        })
    }

    /// The shared database itself, which is what a session outside a transaction
    /// reads and writes.
    #[must_use]
    pub fn committed(&self) -> &Arc<DatabaseEngine> {
        &self.committed
    }

    /// Open a transaction: a copy of the database as it stands now, the version
    /// it stands at, and the schema that copy declares — which is what tells a
    /// later statement of this transaction whether it declared anything.
    ///
    /// BLOCKING (engine work), like everything else here.
    pub fn begin(&self, view: &mut EngineView) -> Result<OpenTransaction, EngineError> {
        // The version is read while the copy is made, so the snapshot and the
        // version it is a snapshot OF cannot drift apart.
        let applied = self.applied.lock().unwrap_or_else(PoisonError::into_inner);
        let snapshot = self.committed.copy()?;
        let at = applied.version;
        drop(applied);
        let catalog = catalog_of(&snapshot, view);
        Ok(OpenTransaction {
            snapshot: Arc::new(snapshot),
            at,
            changed: BTreeSet::new(),
            created: BTreeSet::new(),
            catalog,
            outgrew_its_bounds: false,
        })
    }

    /// Run one statement OUTSIDE any transaction: on the shared database itself,
    /// as its own commit.
    ///
    /// BLOCKING.
    pub fn run_committed(
        &self,
        view: &mut EngineView,
        sql: &str,
    ) -> Result<ResultSet, EngineError> {
        let mut applied = self.applied.lock().unwrap_or_else(PoisonError::into_inner);
        let (result, writes) = self.committed.execute_recording(view, sql)?;
        if !writes.rows.is_empty() {
            let version = applied.version + 1;
            applied.version = version;
            applied.remember(
                version,
                writes.rows.into_iter().map(|record| record.row).collect(),
            );
        }
        Ok(result)
    }

    /// Commit `transaction`: refuse it when another commit has changed a row it
    /// changed, and otherwise apply its own writes to the shared database.
    ///
    /// `Err` with [`EngineErrorKind::SerializationFailure`] is the refusal, and
    /// the transaction then leaves NO effect: its copy is dropped, and the
    /// engine transaction the application ran in is rolled back. A transaction
    /// that outgrew its own bounds is refused the same way it was when it did:
    /// the account of what it changed is no longer whole, so nothing of it is
    /// carried over.
    ///
    /// BLOCKING.
    pub fn commit(
        &self,
        transaction: OpenTransaction,
        view: &mut EngineView,
    ) -> Result<(), EngineError> {
        if transaction.outgrew_its_bounds {
            return Err(too_many_rows());
        }
        let mut applied = self.applied.lock().unwrap_or_else(PoisonError::into_inner);
        if applied.conflicts_with(transaction.at, &transaction.conflict_rows()) {
            return Err(conflict());
        }
        if transaction.changed.is_empty() && !transaction.declared_anything(view) {
            return Ok(());
        }
        let mut carried: BTreeSet<ChangedRow> = BTreeSet::new();
        let applying = self.apply(&transaction, view, &mut carried);
        match applying {
            Ok(()) => {
                let version = applied.version + 1;
                applied.version = version;
                applied.remember(version, carried);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Carry the transaction's own writes over to the shared database, inside
    /// ONE engine transaction, collecting the rows they changed there.
    ///
    /// What is carried over is what the transaction's own copy HOLDS, not the
    /// statements that put it there: for every row it changed, the shared
    /// database ends up holding the value that copy holds — the row as the
    /// transaction left it, or no row at all where it deleted one. So a write
    /// whose value the transaction computed from the database as it stood at its
    /// snapshot commits THAT value, which is what the isolation it runs at
    /// means. The SCHEMA it declared is carried over as the statements that
    /// declared it ([`OpenTransaction::schema_log`]), because a declaration is
    /// the only account of itself an engine keeps.
    ///
    /// A row the transaction CREATED is ADDED rather than put at the row id its
    /// own copy gave it: the engine assigns a row id per database, so another
    /// writer may hold that id, and a row of theirs is not this transaction's to
    /// replace. Two transactions that really created the same row then meet the
    /// database's own KEY here, and that is the conflict
    /// [`EngineErrorKind::SerializationFailure`] reports — the client's statement
    /// was good against the database it ran on.
    ///
    /// Two passes over the rows, because a row's old value can be the value
    /// another of them holds now: every changed row the database already held is
    /// removed first, and then the ones the copy holds are put in.
    fn apply(
        &self,
        transaction: &OpenTransaction,
        view: &mut EngineView,
        carried: &mut BTreeSet<ChangedRow>,
    ) -> Result<(), EngineError> {
        self.committed.execute_as(view, "BEGIN IMMEDIATE")?;
        let carrying = self.carry(transaction, view, carried);
        if let Err(error) = carrying {
            let _ = self.committed.execute_as(view, "ROLLBACK");
            return Err(match error.kind {
                EngineErrorKind::UniqueViolation
                | EngineErrorKind::ForeignKeyViolation
                | EngineErrorKind::CheckViolation
                | EngineErrorKind::NotNullViolation
                | EngineErrorKind::DuplicateObject => conflict(),
                _ => error,
            });
        }
        if let Err(error) = self.committed.execute_as(view, "COMMIT") {
            let _ = self.committed.execute_as(view, "ROLLBACK");
            return Err(error);
        }
        Ok(())
    }

    /// The schema and the rows of [`Self::apply`], inside the engine transaction
    /// it opened.
    ///
    /// The schema is carried as the ENGINE's own catalog holds it, never as the
    /// statements that declared it: an object the transaction declared is created
    /// from the definition the engine keeps for it, which carries no
    /// data-producing query (a `CREATE TABLE … AS SELECT` is held as the plain
    /// declaration of the table it made), and an object it dropped is dropped.
    /// The ROWS of a declared relation are then the ones carried below, like
    /// every other row — so nothing is re-run and nothing is carried twice.
    ///
    /// A definition the transaction CHANGED for a relation the database already
    /// held is refused ([`changed_definition`]): the engine's catalog holds the
    /// definition that relation has NOW and no account of the change itself, and
    /// re-declaring it here would drop the rows of that relation this transaction
    /// never touched.
    fn carry(
        &self,
        transaction: &OpenTransaction,
        view: &mut EngineView,
        carried: &mut BTreeSet<ChangedRow>,
    ) -> Result<(), EngineError> {
        let declared = catalog_of(&transaction.snapshot, view);
        let delta = transaction.catalog.delta(&declared)?;
        // A relation this transaction DECLARED holds, in its own copy, the rows
        // the engine reported it writing — and nothing else. A statement that
        // declared a relation and FILLED it in one go writes rows the engine's
        // update hook does not report, so the account of them is not whole and
        // the commit is refused rather than carrying over the part of it that is
        // ([`unaccounted_change`]).
        for object in delta.declared.iter().filter(|object| object.kind == "table") {
            if transaction.holds_rows_nobody_accounted_for(view, &object.name)? {
                return Err(unaccounted_change());
            }
        }
        // What it dropped, in the reverse of the order the objects were
        // declared in, so a relation goes after what was declared on it.
        for object in delta.dropped.iter().rev() {
            self.committed.execute_as(
                view,
                &format!(
                    "DROP {} IF EXISTS {}",
                    object.kind.to_ascii_uppercase(),
                    quoted(&object.name)
                ),
            )?;
        }
        // ... and what it declared, in the order it declared them, so an index
        // or a trigger goes after the relation it is on.
        for object in &delta.declared {
            self.committed.execute_as(view, &object.definition)?;
        }
        // The rows the database ALREADY HELD, taken out first.
        for (table, rowid) in &transaction.changed {
            if transaction.created.contains(&(table.clone(), *rowid)) {
                continue;
            }
            let (_result, writes) = self.committed.execute_recording(
                view,
                &format!("DELETE FROM {} WHERE rowid = {rowid}", quoted(table)),
            )?;
            carried.extend(writes.rows.into_iter().map(|record| record.row));
        }
        // ... and then the rows the transaction's own copy holds for them.
        for (table, rowid) in &transaction.changed {
            let Some(row) = transaction.row_as_it_stands(view, table, *rowid)? else {
                // A row it deleted, or one it created and then deleted again:
                // there is nothing to put in.
                continue;
            };
            let created = transaction.created.contains(&(table.clone(), *rowid));
            let (_result, writes) = self
                .committed
                .execute_recording(view, &insert_of(table, *rowid, &row, created))?;
            carried.extend(writes.rows.into_iter().map(|record| record.row));
        }
        Ok(())
    }
}

/// One object of a database's catalog, as the ENGINE holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclaredObject {
    /// Its name.
    name: String,
    /// What kind of object it is, as the catalog classifies it (`table`,
    /// `index`, `view`, `trigger`).
    kind: String,
    /// The definition the engine keeps for it, which is what declares it again
    /// in another database. Empty for an object the engine declared ITSELF (the
    /// index behind a key), which its relation's own definition brings with it.
    definition: String,
}

/// What a database DECLARES, in the order the engine holds it, which is the
/// order the objects were declared in.
#[derive(Debug, Clone, Default)]
struct Catalog {
    objects: Vec<DeclaredObject>,
}

/// The difference between two catalogs: what was declared since, and what was
/// dropped.
#[derive(Debug, Default)]
struct CatalogDelta {
    declared: Vec<DeclaredObject>,
    dropped: Vec<DeclaredObject>,
}

impl Catalog {
    /// The object named `name`, where this catalog holds one.
    fn object(&self, name: &str) -> Option<&DeclaredObject> {
        self.objects.iter().find(|object| object.name == name)
    }

    /// What `now` declares that this catalog did not, and what it no longer
    /// declares — the schema a commit carries over.
    ///
    /// `Err` when a definition CHANGED, which is the one schema change this
    /// module has no whole account of ([`changed_definition`]), and when the
    /// difference is larger than one transaction carries
    /// ([`MAX_TRANSACTION_DECLARATIONS`]).
    fn delta(&self, now: &Catalog) -> Result<CatalogDelta, EngineError> {
        let mut delta = CatalogDelta::default();
        for object in &now.objects {
            match self.object(&object.name) {
                Some(held) if held.definition != object.definition => {
                    return Err(changed_definition(&object.name))
                }
                Some(_) => {}
                None if object.definition.is_empty() => {}
                None => delta.declared.push(object.clone()),
            }
        }
        for object in &self.objects {
            if now.object(&object.name).is_none() && !object.definition.is_empty() {
                delta.dropped.push(object.clone());
            }
        }
        if delta.declared.len() + delta.dropped.len() > MAX_TRANSACTION_DECLARATIONS {
            return Err(EngineError::new(
                EngineErrorKind::LimitExceeded,
                format!(
                    "a transaction may declare or drop {MAX_TRANSACTION_DECLARATIONS} objects \
                     before it commits; commit it in smaller transactions"
                ),
            ));
        }
        Ok(delta)
    }
}

/// What a database DECLARES, read off the engine's own catalog in the order it
/// holds its objects.
fn catalog_of(engine: &DatabaseEngine, view: &mut EngineView) -> Catalog {
    let answered = engine.execute_as(
        view,
        "SELECT name, type, coalesce(sql, '') FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
    );
    let Ok(answered) = answered else {
        return Catalog::default();
    };
    let text = |cell: Option<&Cell>| match cell {
        Some(Cell::Text(text)) => Some(text.clone()),
        _ => None,
    };
    Catalog {
        objects: answered
            .rows
            .iter()
            .filter_map(|row| {
                Some(DeclaredObject {
                    name: text(row.first())?,
                    kind: text(row.get(1))?,
                    definition: text(row.get(2))?,
                })
            })
            .collect(),
    }
}

/// One row as a database holds it: the columns it declares and the values this
/// row carries.
#[derive(Debug, Clone)]
struct HeldRow {
    columns: Vec<String>,
    values: Vec<Cell>,
}

/// The `INSERT` that puts `row` into `table`.
///
/// A row the database already held is put back AT its own row id, which is what
/// changing it means; a row the transaction CREATED is added without one, so the
/// engine assigns it an id of its own in the database it is being added to.
fn insert_of(table: &str, rowid: i64, row: &HeldRow, created: bool) -> String {
    let mut columns: Vec<String> = row.columns.iter().map(|name| quoted(name)).collect();
    let mut values: Vec<String> = row.values.iter().map(literal).collect();
    if !created {
        columns.insert(0, "rowid".to_string());
        values.insert(0, rowid.to_string());
    }
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quoted(table),
        columns.join(", "),
        values.join(", ")
    )
}

/// `name` as one quoted identifier, so a name carrying a character the grammar
/// reads otherwise is still the name.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// One value as the SQL literal the engine reads it back from, in the storage
/// class it holds: an integer and a real as themselves (the real in the shortest
/// form that reads back to the same value), text quoted, a blob as the hex
/// literal the engine writes one as, and a null as `NULL`.
fn literal(cell: &Cell) -> String {
    match cell {
        Cell::Null => "NULL".to_string(),
        Cell::Integer(number) => number.to_string(),
        Cell::Real(number) => format!("{number:?}"),
        Cell::Text(text) => format!("'{}'", text.replace('\'', "''")),
        Cell::Blob(bytes) => format!("X'{}'", ::hex::encode(bytes)),
    }
}


/// The transaction ONE session has open: the copy of the database it runs on,
/// the version that copy was taken at, the rows it has changed and the writes a
/// commit carries over.
///
/// Dropping it is the whole of rolling it back.
#[derive(Debug)]
pub struct OpenTransaction {
    /// The database as it stood when this transaction opened, which is what its
    /// statements run on.
    snapshot: Arc<DatabaseEngine>,
    /// The version of the shared database that copy was taken at.
    at: u64,
    /// The rows it has changed, by the row id the snapshot holds them at.
    changed: BTreeSet<ChangedRow>,
    /// Of those, the rows that were NOT in the database when this transaction
    /// opened, which are the ones not compared by row id.
    created: BTreeSet<ChangedRow>,
    /// What the database DECLARED when this transaction opened, which is what a
    /// commit compares the copy's own catalog against to carry the schema over.
    catalog: Catalog,
    /// Whether this transaction outgrew one of its own bounds, which is what
    /// stops it committing on an account of its writes that is no longer whole.
    outgrew_its_bounds: bool,
}

impl OpenTransaction {
    /// The database this transaction's statements run on, for a caller that has
    /// to put something on the engine before a statement does (a catalog view
    /// the statement reads).
    #[must_use]
    pub fn engine(&self) -> &Arc<DatabaseEngine> {
        &self.snapshot
    }

    /// Run one statement inside this transaction: on its own copy of the
    /// database, recording what it changed.
    ///
    /// What is recorded is the ROWS the engine reported changed, which with the
    /// copy's own catalog is the whole of what a commit carries over
    /// ([`OptimisticWriters::carry`]) — no statement of the client's is kept, and
    /// none is ever re-run.
    ///
    /// A statement that would take this transaction past the rows one
    /// transaction may change is refused, and so is one whose change the engine
    /// reports no account of at all, because a change this module cannot carry
    /// over must not look committed. The transaction is then one that can no
    /// longer commit ([`OptimisticWriters::commit`]).
    ///
    /// BLOCKING.
    pub fn run(&mut self, view: &mut EngineView, sql: &str) -> Result<ResultSet, EngineError> {
        let (result, writes) = self.snapshot.execute_recording(view, sql)?;
        if !writes.wrote {
            return Ok(result);
        }
        if self.changed.len().saturating_add(writes.rows.len()) > MAX_TRANSACTION_ROWS {
            self.outgrew_its_bounds = true;
            return Err(too_many_rows());
        }
        // A statement that changed rows the engine reported none of is one whose
        // writes cannot be carried over — a relation declared WITHOUT ROWID,
        // whose changes an engine's update hook does not report, and a statement
        // that DECLARES a relation and fills it in one go, whose rows the engine
        // writes without reporting them either. It is refused rather than
        // committed as the rows nobody accounted for.
        if result.affected > 0 && writes.rows.is_empty() {
            self.outgrew_its_bounds = true;
            return Err(unaccounted_change());
        }
        for record in writes.rows {
            // A row is this transaction's OWN only the first time the
            // transaction touches it, and only when the engine reported it as an
            // insert: a row the database already held, deleted and then inserted
            // again at the same row id, is still that row — which is what makes
            // a commit replace it rather than add a second one.
            if record.created && !self.changed.contains(&record.row) {
                self.created.insert(record.row.clone());
            }
            self.changed.insert(record.row);
        }
        Ok(result)
    }

    /// Whether this transaction has DECLARED or dropped anything, which is the
    /// other half of what a commit carries over beside the rows.
    fn declared_anything(&self, view: &mut EngineView) -> bool {
        match self.catalog.delta(&catalog_of(&self.snapshot, view)) {
            Ok(delta) => !delta.declared.is_empty() || !delta.dropped.is_empty(),
            // A definition it CHANGED, which the commit refuses: there is
            // something to carry over, and that is where it is answered.
            Err(_) => true,
        }
    }

    /// Whether the relation `table` holds, in this transaction's own copy, more
    /// rows than the engine reported this transaction writing to it — which is a
    /// relation that was declared and FILLED by one statement, whose rows the
    /// engine writes without reporting them.
    fn holds_rows_nobody_accounted_for(
        &self,
        view: &mut EngineView,
        table: &str,
    ) -> Result<bool, EngineError> {
        let answered = self.snapshot.execute_as(
            view,
            &format!("SELECT count(*) FROM {}", quoted(table)),
        )?;
        let held = match answered.rows.first().and_then(|row| row.first()) {
            Some(Cell::Integer(count)) => *count,
            _ => 0,
        };
        let accounted = self
            .changed
            .iter()
            .filter(|(changed, _)| &**changed == table)
            .count();
        Ok(held > i64::try_from(accounted).unwrap_or(i64::MAX))
    }

    /// The rows a conflict is decided on: the rows this transaction changed that
    /// the database ALREADY HELD when its snapshot was taken.
    ///
    /// A row it created is left out: the engine assigns a row id per database,
    /// so the id of a new row says nothing about which row another transaction's
    /// new row is. Two created rows that are really the same row are settled by
    /// the database's own key when the commit is applied
    /// ([`OptimisticWriters::apply`]).
    fn conflict_rows(&self) -> BTreeSet<ChangedRow> {
        self.changed.difference(&self.created).cloned().collect()
    }

    /// The row `table`.`rowid` as this transaction's own copy holds it NOW, and
    /// `None` where the copy holds no such row — a row it deleted, or one whose
    /// table it dropped.
    fn row_as_it_stands(
        &self,
        view: &mut EngineView,
        table: &str,
        rowid: i64,
    ) -> Result<Option<HeldRow>, EngineError> {
        let answered = match self.snapshot.execute_as(
            view,
            &format!("SELECT * FROM {} WHERE rowid = {rowid}", quoted(table)),
        ) {
            Ok(answered) => answered,
            // The table itself is gone from the copy, which the schema delta
            // carries over as the drop it was.
            Err(error) if error.kind == EngineErrorKind::UndefinedObject => return Ok(None),
            Err(error) => return Err(error),
        };
        let Some(values) = answered.rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(HeldRow {
            columns: answered
                .columns
                .iter()
                .map(|column| column.name.clone())
                .collect(),
            values,
        }))
    }
}

/// The refusal of a commit whose rows another transaction changed first.
fn conflict() -> EngineError {
    EngineError::new(
        EngineErrorKind::SerializationFailure,
        CONFLICT_MESSAGE.to_string(),
    )
}

/// The refusal of a change to the DEFINITION of a relation the database already
/// held, made inside a transaction.
///
/// The engine's catalog holds the definition that relation has NOW and no
/// account of the change that got it there, so carrying it over would mean
/// declaring the relation again — which would drop the rows of it this
/// transaction never touched. The change is refused instead.
fn changed_definition(name: &str) -> EngineError {
    EngineError::new(
        EngineErrorKind::FeatureNotSupported,
        format!(
            "this endpoint cannot commit a change to the definition of \"{name}\", which the \
             database already held when the transaction began"
        ),
    )
}

/// The refusal of a change this module has no account of, so cannot carry over:
/// a relation declared without row ids, and a statement that declares a relation
/// and fills it in one go — the engine's update hook reports the rows of neither,
/// and a commit carries over only the rows it reported.
fn unaccounted_change() -> EngineError {
    EngineError::new(
        EngineErrorKind::FeatureNotSupported,
        "this endpoint cannot commit a change it has no account of: a relation declared without \
         row ids, or one declared and filled by the same statement"
            .to_string(),
    )
}

/// The refusal of a transaction that has changed more rows than one transaction
/// of this endpoint holds ([`MAX_TRANSACTION_ROWS`]).
fn too_many_rows() -> EngineError {
    EngineError::new(
        EngineErrorKind::LimitExceeded,
        format!(
            "a transaction may change {MAX_TRANSACTION_ROWS} rows before it commits; commit it \
             in smaller transactions"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_wire_planes::sql_engine::Cell;

    /// A session view for a statement on the endpoint's one database.
    fn view() -> EngineView {
        EngineView {
            database: "appdb".to_string(),
            user: "dbadmin".to_string(),
            ..EngineView::default()
        }
    }

    /// The one value a single-row, single-column answer carries.
    fn only(result: &ResultSet) -> Option<String> {
        match result.rows.first()?.first()? {
            Cell::Text(text) => Some(text.clone()),
            Cell::Integer(number) => Some(number.to_string()),
            _ => None,
        }
    }

    /// An endpoint's database with one table and one row in it.
    fn seeded() -> Arc<OptimisticWriters> {
        let writers = OptimisticWriters::on(Arc::new(
            DatabaseEngine::open().expect("the engine starts"),
        ));
        let mut view = view();
        for statement in [
            "CREATE TABLE t (id INTEGER PRIMARY KEY, who TEXT)",
            "INSERT INTO t (id, who) VALUES (1, 'before')",
        ] {
            writers
                .run_committed(&mut view, statement)
                .expect("the seed runs");
        }
        writers
    }

    /// RULE: two transactions that change the SAME row both run without waiting,
    /// and exactly one of the commits stands — the other is refused
    /// `serialization_failure` and leaves no effect.
    #[test]
    fn two_writers_of_one_row_leave_exactly_one_commit_standing() {
        let writers = seeded();
        let mut view = view();
        let mut first = writers.begin(&mut view).expect("the first transaction opens");
        let mut second = writers.begin(&mut view).expect("the second transaction opens");
        // Neither statement waits for the other: both answer their own copy.
        for (case, transaction, value) in [
            ("the first writer", &mut first, "first"),
            ("the second writer", &mut second, "second"),
        ] {
            let answered = transaction.run(
                &mut view,
                &format!("UPDATE t SET who = '{value}' WHERE id = 1"),
            );
            let changed = answered.as_ref().map(|result| result.affected);
            assert_eq!(
                changed.ok(),
                Some(1),
                "case {case:?}: the UPDATE must change its own copy's row, got {answered:?}"
            );
        }
        // A third reader still sees the row as it was: neither writer has
        // committed.
        let read = writers
            .run_committed(&mut view, "SELECT who FROM t WHERE id = 1")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("before"),
            "case the uncommitted window: a reader must see the row as it was, got {read:?}"
        );

        writers
            .commit(first, &mut view)
            .expect("the first commit stands");
        let refused = writers
            .commit(second, &mut view)
            .expect_err("the second commit is refused");
        assert_eq!(
            refused.kind,
            EngineErrorKind::SerializationFailure,
            "case the losing commit: expected a serialization failure, got {refused:?}"
        );
        let read = writers
            .run_committed(&mut view, "SELECT who FROM t WHERE id = 1")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("first"),
            "case after the refusal: only the winner's write may stand, got {read:?}"
        );

        // Re-run serially and it commits: nothing about the refusal is sticky.
        let mut retried = writers.begin(&mut view).expect("the retry opens");
        retried
            .run(&mut view, "UPDATE t SET who = 'second' WHERE id = 1")
            .expect("the retry's UPDATE runs");
        writers
            .commit(retried, &mut view)
            .expect("the serial retry commits");
        let read = writers
            .run_committed(&mut view, "SELECT who FROM t WHERE id = 1")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("second"),
            "case the serial retry: it must stand, got {read:?}"
        );
    }

    /// RULE: a conflict is between WRITERS of the same row. Transactions that
    /// write DIFFERENT rows both commit; two that create the SAME row — which the
    /// database's own key decides — leave exactly one standing.
    #[test]
    fn only_writers_of_the_same_row_conflict() {
        let writers = seeded();
        let mut view = view();
        let mut left = writers.begin(&mut view).expect("a transaction opens");
        let mut right = writers.begin(&mut view).expect("a transaction opens");
        left.run(&mut view, "INSERT INTO t (id, who) VALUES (10, 'left')")
            .expect("the INSERT runs");
        right
            .run(&mut view, "INSERT INTO t (id, who) VALUES (11, 'right')")
            .expect("the INSERT runs");
        writers.commit(left, &mut view).expect("the first commits");
        writers
            .commit(right, &mut view)
            .expect("a write of another row commits too");
        let read = writers
            .run_committed(&mut view, "SELECT count(*) FROM t")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("3"),
            "case distinct rows: both writes must stand, got {read:?}"
        );

        let mut left = writers.begin(&mut view).expect("a transaction opens");
        let mut right = writers.begin(&mut view).expect("a transaction opens");
        for (case, transaction, who) in [
            ("the first creator", &mut left, "left"),
            ("the second creator", &mut right, "right"),
        ] {
            let answered =
                transaction.run(&mut view, &format!("INSERT INTO t (id, who) VALUES (20, '{who}')"));
            assert!(
                answered.is_ok(),
                "case {case:?}: the INSERT must run on its own copy, got {answered:?}"
            );
        }
        writers.commit(left, &mut view).expect("the first commits");
        let refused = writers
            .commit(right, &mut view)
            .expect_err("the second creator of one row is refused");
        assert_eq!(
            refused.kind,
            EngineErrorKind::SerializationFailure,
            "case the same new row: expected a serialization failure, got {refused:?}"
        );
        let read = writers
            .run_committed(&mut view, "SELECT who FROM t WHERE id = 20")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("left"),
            "case the same new row: only the winner's row may stand, got {read:?}"
        );
    }

    /// RULE: a transaction is refused for the rows it WROTE and for nothing else,
    /// and what it COMMITS is the value its own statement computed against the
    /// database as it stood at its snapshot — not a value recomputed when the
    /// commit lands. A write whose value it read from rows another transaction
    /// changed meanwhile is therefore not a conflict, and the row it committed is
    /// the row the client saw inside the transaction.
    #[test]
    fn a_commit_carries_the_value_the_transaction_computed_on_its_snapshot() {
        let writers = seeded();
        let mut view = view();
        for statement in [
            "CREATE TABLE source (id INTEGER PRIMARY KEY, n INTEGER)",
            "INSERT INTO source (id, n) VALUES (1, 1), (2, 2)",
            "CREATE TABLE total (id INTEGER PRIMARY KEY, n INTEGER)",
        ] {
            writers
                .run_committed(&mut view, statement)
                .expect("the seed runs");
        }
        // A transaction whose own write READS a table, and another that changes
        // that table and commits first.
        let mut computing = writers.begin(&mut view).expect("a transaction opens");
        computing
            .run(
                &mut view,
                "INSERT INTO total (id, n) SELECT 1, sum(n) FROM source",
            )
            .expect("the write runs on its own copy");
        let computed = computing
            .run(&mut view, "SELECT n FROM total WHERE id = 1")
            .expect("the read runs");
        assert_eq!(
            only(&computed).as_deref(),
            Some("3"),
            "case inside the transaction: it computes over its own snapshot, got {computed:?}"
        );
        let mut other = writers.begin(&mut view).expect("another transaction opens");
        other
            .run(&mut view, "INSERT INTO source (id, n) VALUES (3, 10)")
            .expect("the INSERT runs");
        writers.commit(other, &mut view).expect("it commits first");
        // No conflict: the rows the first transaction WROTE are its own.
        writers
            .commit(computing, &mut view)
            .expect("a write that only READ the changed rows is not refused");
        let read = writers
            .run_committed(&mut view, "SELECT n FROM total WHERE id = 1")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("3"),
            "case the committed value: it must be the one the transaction computed on its \
             snapshot, got {read:?}"
        );
    }

    /// RULE: a statement whose change the engine reports NO account of — one that
    /// DECLARES a relation and fills it in the same statement, whose rows the
    /// engine writes without reporting them — leaves a transaction that cannot
    /// commit: the commit is refused rather than carrying over the part of it
    /// that was accounted for. The same work written as a declaration and an
    /// `INSERT` commits, with each row carried over once from the transaction's
    /// own copy.
    #[test]
    fn a_statement_that_declares_and_fills_a_relation_is_refused_rather_than_half_carried() {
        let writers = seeded();
        let mut view = view();
        writers
            .run_committed(&mut view, "INSERT INTO t (id, who) VALUES (2, 'also')")
            .expect("the seed runs");

        let mut declaring = writers.begin(&mut view).expect("a transaction opens");
        declaring
            .run(&mut view, "CREATE TABLE copied AS SELECT id, who FROM t")
            .expect("the engine takes the statement on the transaction's own copy");
        let refused = writers
            .commit(declaring, &mut view)
            .expect_err("a declare-and-fill statement leaves a transaction that cannot commit");
        assert_eq!(
            refused.kind,
            EngineErrorKind::FeatureNotSupported,
            "case the declare-and-fill: expected the refusal of a change with no account, got \
             {refused:?}"
        );
        let held = writers.run_committed(&mut view, "SELECT count(*) FROM copied");
        assert!(
            held.is_err(),
            "case after the refusal: the relation must not be there at all, got {held:?}"
        );

        // The same work, written as a declaration and an INSERT: each row is
        // carried over once, as the transaction's own copy held it, even though
        // another transaction changed the source and committed first.
        let mut writing = writers.begin(&mut view).expect("a transaction opens");
        for statement in [
            "CREATE TABLE copied (id INTEGER PRIMARY KEY, who TEXT)",
            "INSERT INTO copied (id, who) SELECT id, who FROM t",
        ] {
            writing
                .run(&mut view, statement)
                .unwrap_or_else(|error| panic!("case {statement:?}: it must run, got {error:?}"));
        }
        let mut other = writers.begin(&mut view).expect("another transaction opens");
        other
            .run(&mut view, "INSERT INTO t (id, who) VALUES (3, 'later')")
            .expect("the INSERT runs");
        writers.commit(other, &mut view).expect("it commits first");
        writers
            .commit(writing, &mut view)
            .expect("the declaration and its rows commit");
        let read = writers
            .run_committed(&mut view, "SELECT count(*) FROM copied")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("2"),
            "case the declaration and its rows: the relation must hold the rows its snapshot \
             had, once each, got {read:?}"
        );
    }

    /// RULE: a row the database already held, deleted and put back inside one
    /// transaction, is still THAT row — a commit replaces it rather than adding a
    /// second one.
    #[test]
    fn a_row_deleted_and_put_back_in_one_transaction_is_still_that_row() {
        let writers = seeded();
        let mut view = view();
        let mut transaction = writers.begin(&mut view).expect("a transaction opens");
        for statement in [
            "DELETE FROM t WHERE id = 1",
            "INSERT INTO t (id, who) VALUES (1, 'again')",
        ] {
            transaction
                .run(&mut view, statement)
                .unwrap_or_else(|error| panic!("case {statement:?}: it must run, got {error:?}"));
        }
        writers
            .commit(transaction, &mut view)
            .expect("the commit stands");
        for (case, statement, expected) in [
            ("the rows the table holds", "SELECT count(*) FROM t", "1"),
            ("the row itself", "SELECT who FROM t WHERE id = 1", "again"),
        ] {
            let read = writers
                .run_committed(&mut view, statement)
                .unwrap_or_else(|error| panic!("case {case:?}: the read must run, got {error:?}"));
            assert_eq!(
                only(&read).as_deref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// RULE: a transaction reads the database as it stood when it OPENED, however
    /// often anybody else commits — and a transaction that is dropped rather than
    /// committed leaves nothing behind.
    #[test]
    fn a_transaction_reads_its_own_snapshot_and_a_dropped_one_leaves_nothing() {
        let writers = seeded();
        let mut view = view();
        let mut held = writers.begin(&mut view).expect("a transaction opens");
        // Somebody else commits a row AFTER this transaction opened.
        let mut other = writers.begin(&mut view).expect("another transaction opens");
        other
            .run(&mut view, "INSERT INTO t (id, who) VALUES (2, 'later')")
            .expect("the INSERT runs");
        writers.commit(other, &mut view).expect("it commits");
        let read = held
            .run(&mut view, "SELECT count(*) FROM t")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("1"),
            "case the snapshot: the open transaction must not see a later commit, got {read:?}"
        );

        // What it wrote and never committed is nowhere.
        held.run(&mut view, "UPDATE t SET who = 'never' WHERE id = 1")
            .expect("the UPDATE runs");
        drop(held);
        let read = writers
            .run_committed(&mut view, "SELECT who FROM t WHERE id = 1")
            .expect("the read runs");
        assert_eq!(
            only(&read).as_deref(),
            Some("before"),
            "case the dropped transaction: it must leave the row as it was, got {read:?}"
        );
    }

    /// RULE: a commit carries the transaction's writes over as ONE step — the
    /// schema it declared and every row it wrote, or nothing at all.
    #[test]
    fn a_commit_carries_the_whole_transaction_over() {
        let writers = seeded();
        let mut view = view();
        let mut transaction = writers.begin(&mut view).expect("a transaction opens");
        for statement in [
            "CREATE TABLE orders (id INTEGER PRIMARY KEY, note TEXT)",
            "INSERT INTO orders (id, note) VALUES (1, 'one')",
            "INSERT INTO orders (id, note) VALUES (2, 'two')",
            "DELETE FROM t WHERE id = 1",
        ] {
            transaction
                .run(&mut view, statement)
                .unwrap_or_else(|error| panic!("case {statement:?}: it must run, got {error:?}"));
        }
        // Nothing of it is visible before the commit.
        let before = writers.run_committed(&mut view, "SELECT count(*) FROM orders");
        assert!(
            before.is_err(),
            "case before the commit: the shared database must not hold the new table, got \
             {before:?}"
        );
        writers
            .commit(transaction, &mut view)
            .expect("the commit stands");
        for (case, statement, expected) in [
            ("the rows it wrote", "SELECT count(*) FROM orders", "2"),
            ("the row it deleted", "SELECT count(*) FROM t", "0"),
        ] {
            let read = writers
                .run_committed(&mut view, statement)
                .unwrap_or_else(|error| panic!("case {case:?}: {statement} must run, got {error:?}"));
            assert_eq!(
                only(&read).as_deref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
