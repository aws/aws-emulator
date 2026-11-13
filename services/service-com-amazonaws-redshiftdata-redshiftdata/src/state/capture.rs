// SPDX-License-Identifier: CC0-1.0

//! Taking and restoring a snapshot of a warehouse's databases.
//!
//! A control plane that snapshots a data warehouse (`CreateSnapshot`,
//! `CreateClusterSnapshot`) and one that writes a snapshot back
//! (`RestoreFromSnapshot`, `RestoreFromClusterSnapshot`) reach the databases
//! through this module, because the SQL engine those databases live in belongs
//! to this crate: what they hold is an opaque [`WarehouseCapture`], and the
//! copying itself is the engine's own page-level copy.

use super::*;

/// How many pages the engine's own copy moves per step.
///
/// Small deliberately: the deadline is checked BETWEEN steps, so the step size is
/// how often a copy can be given up on. A page is 4 KiB, so a step is a megabyte
/// of copying — short enough to be a cancellation point, long enough not to pay
/// the per-step cost per page.
const COPY_PAGES_PER_STEP: i32 = 256;

/// Wall-clock bound on ONE whole warehouse copy, from the moment the blocking task
/// starts: it bounds both the wait for a database a statement is running on and the
/// copying itself, so a copy can never run unbounded on a blocking-pool thread.
/// Generous beside [`MAX_ENGINE_DEADLINE`], which is how long a statement can be
/// holding a database this copy needs.
pub const COPY_TIMEOUT: Duration = MAX_ENGINE_DEADLINE.saturating_add(Duration::from_secs(8));

/// How long a REQUEST waits for a copy: [`COPY_TIMEOUT`] plus a grace period, so
/// what ends a copy is its own deadline and not the request's patience.
pub const COPY_REQUEST_BUDGET: Duration = COPY_TIMEOUT.saturating_add(Duration::from_secs(3));

/// How long the copy sleeps between attempts on a database another statement is
/// running on. It runs on a blocking-pool thread, never on an async worker, so
/// sleeping here blocks nothing but this copy.
const COPY_LOCK_POLL: Duration = Duration::from_millis(10);

/// One database of a capture: its name in the warehouse, and the copy itself.
#[derive(Debug)]
struct CapturedDatabase {
    /// The `Database` name the warehouse holds this database under.
    database: String,
    /// The copy, a database of its own — so it is unaffected by later statements
    /// against the warehouse it was copied from, and by that warehouse being
    /// deleted.
    handle: DatabaseHandle,
}

/// The databases of one warehouse as they stood at one instant.
///
/// Redshift Serverless' `CreateSnapshot` takes one ("creates a snapshot of all
/// databases in a namespace") and its `RestoreFromSnapshot` writes it back over a
/// workgroup's databases; Amazon Redshift's `CreateClusterSnapshot` takes one of a
/// provisioned cluster ("a snapshot contains a copy of the cluster data as of this
/// exact time") and `RestoreFromClusterSnapshot` writes it into the cluster it
/// creates. The copy is taken HERE, in the crate that owns the SQL
/// engine the databases live in, and handed to the control plane as an opaque
/// value: nothing outside this module can read or alter a captured database.
///
/// Its memory is bounded by the same `max_page_count` as the databases it copies
/// (see [`pages`](Self::pages), which the holder uses to bound how much it retains).
#[derive(Debug, Default)]
pub struct WarehouseCapture {
    /// One copy per captured database, in database-name order.
    databases: Vec<CapturedDatabase>,
    /// The engine pages the copies occupy, as the engine counted them.
    pages: i64,
}

impl WarehouseCapture {
    /// The names of the databases this capture holds, in name order.
    pub fn database_names(&self) -> Vec<&str> {
        self.databases
            .iter()
            .map(|captured| captured.database.as_str())
            .collect()
    }

    /// How many engine pages the capture occupies — the size the engine itself
    /// reports for the copies, in the unit `max_page_count` bounds a database in.
    /// A holder of many captures bounds what it retains with this.
    pub fn pages(&self) -> i64 {
        self.pages
    }

    /// True when the warehouse held no database to copy, so the capture carries no
    /// data. A namespace nothing ran a statement against captures empty.
    pub fn is_empty(&self) -> bool {
        self.databases.is_empty()
    }
}

/// Why a warehouse could not be captured or restored.
#[derive(Debug)]
pub enum WarehouseUnavailable {
    /// The engine given holds no Data API store, so there are no databases to copy.
    StoreUnreachable,
    /// The SQL engine could not open, confine or copy a database.
    Engine(String),
    /// The restore would need more than [`MAX_OPEN_DATABASES`] databases open.
    TooManyOpen(usize),
    /// The copy did not finish within [`COPY_TIMEOUT`] — it was given up on rather
    /// than left running on a blocking-pool thread.
    TimedOut(Duration),
}

impl From<NoDatabase> for WarehouseUnavailable {
    fn from(no_database: NoDatabase) -> Self {
        match no_database {
            NoDatabase::EngineUnavailable(message) => Self::Engine(message),
            NoDatabase::TooManyOpen(open) => Self::TooManyOpen(open),
        }
    }
}

impl std::fmt::Display for WarehouseUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StoreUnreachable => write!(f, "the redshift-data store is unreachable"),
            Self::Engine(message) => write!(f, "the SQL engine could not copy a database: {message}"),
            Self::TooManyOpen(open) => write!(
                f,
                "this emulator holds the most databases it can open at once ({open})"
            ),
            Self::TimedOut(after) => write!(
                f,
                "the copy was given up on after {} seconds",
                after.as_secs()
            ),
        }
    }
}

impl std::error::Error for WarehouseUnavailable {}

/// Copy the databases reachable through `targets` into a capture.
///
/// BLOCKING, deliberately, exactly as [`run_on`] is: it drives the SQL engine, so a
/// caller must put it on a blocking pool (`tokio::task::spawn_blocking`) and bound
/// the wait for it ([`COPY_REQUEST_BUDGET`]) rather than run it on an async worker.
/// Its own deadline is [`COPY_TIMEOUT`], checked while waiting for a database and
/// between copy steps, so it always gives up before the request does.
///
/// The union of those targets' databases, because this emulator's Data API
/// addresses a database by the warehouse a statement named: a Redshift Serverless
/// namespace's data is reached through the workgroups attached to it, and a
/// provisioned cluster's through the cluster itself — so each control plane hands
/// over the targets its own resource is reached through. A database name two of
/// them both hold is captured once, from the first target in the order given.
///
/// The state lock is released before any database is touched — see
/// [`ComAmazonawsRedshiftdataRedshiftdataState::warehouse_handles`].
pub fn capture_warehouse<Eng: Send + Sync + 'static>(
    engine: &Eng,
    targets: &[ConnectionTarget],
) -> Result<WarehouseCapture, WarehouseUnavailable> {
    let Some(state) = service_state(engine) else {
        return Err(WarehouseUnavailable::StoreUnreachable);
    };
    let deadline = Instant::now() + COPY_TIMEOUT;

    let sources: Vec<(String, DatabaseHandle)> = {
        let state = state.lock();
        let mut sources: Vec<(String, DatabaseHandle)> = Vec::new();
        for target in targets {
            for (database, handle) in state.warehouse_handles(target) {
                if !sources.iter().any(|(held, _)| held == &database) {
                    sources.push((database, handle));
                }
            }
        }
        sources
    };

    let mut capture = WarehouseCapture::default();
    for (database, source) in sources {
        // Each copy is opened through `Database::open`, so a captured database
        // carries exactly the confinement a live one does.
        let mut copy = Database::open()
            .map_err(|error| WarehouseUnavailable::Engine(scrub_paths(&error.to_string())))?;
        let locked = lock_database_until(&source, deadline)
            .ok_or(WarehouseUnavailable::TimedOut(COPY_TIMEOUT))?;
        let pages = copy_database(&locked, &mut copy, deadline)?;
        drop(locked);
        capture.pages += pages;
        capture.databases.push(CapturedDatabase {
            database,
            handle: std::sync::Arc::new(Mutex::new(copy)),
        });
    }
    Ok(capture)
}

/// Write `capture` over the databases of `target`, and report how many
/// databases it materialised.
///
/// BLOCKING and bounded exactly as [`capture_warehouse`] is: a blocking pool and
/// [`COPY_REQUEST_BUDGET`] on the caller's side, [`COPY_TIMEOUT`] on its own.
///
/// The warehouse ends up holding exactly the databases the capture holds, with
/// exactly their contents: a restore replaces a warehouse's contents rather than
/// merging into them, so a database the capture does not name is not in the
/// restored warehouse either. A statement submitted after this reads the restored
/// data.
///
/// Nothing the warehouse serves changes until every copy has been made: the
/// capture is copied into FRESH databases first and they replace the warehouse's
/// in one step, so a restore that cannot be completed leaves the warehouse exactly
/// as it found it.
pub fn restore_warehouse<Eng: Send + Sync + 'static>(
    engine: &Eng,
    capture: &WarehouseCapture,
    target: &ConnectionTarget,
) -> Result<usize, WarehouseUnavailable> {
    let Some(state) = service_state(engine) else {
        return Err(WarehouseUnavailable::StoreUnreachable);
    };
    let deadline = Instant::now() + COPY_TIMEOUT;

    // Asked before anything is copied: a restore the emulator has no room for is
    // refused while the warehouse is still untouched.
    state
        .lock()
        .room_for_databases(target, capture.databases.len())?;

    // With the state lock released: one fresh confined database per captured
    // database, with the copy already in it.
    let mut restored: Vec<(String, Database)> = Vec::with_capacity(capture.databases.len());
    for captured in &capture.databases {
        let mut fresh = Database::open()
            .map_err(|error| WarehouseUnavailable::Engine(scrub_paths(&error.to_string())))?;
        // Nothing but a restore ever locks a CAPTURED database, so this wait is
        // bounded for the same reason the others are rather than because it is
        // expected to be contended.
        let source = lock_database_until(&captured.handle, deadline)
            .ok_or(WarehouseUnavailable::TimedOut(COPY_TIMEOUT))?;
        copy_database(&source, &mut fresh, deadline)?;
        drop(source);
        restored.push((captured.database.clone(), fresh));
    }

    // The swap, under the state lock and infallible.
    let count = restored.len();
    state.lock().install_databases(target, restored);
    Ok(count)
}

/// Why a bounded copy of a warehouse's databases did not happen.
///
/// The copy is blocking engine work, so a caller runs it on the blocking pool
/// under a budget ([`capture_warehouse_bounded`], [`restore_warehouse_bounded`])
/// and it can fail three ways: the copy itself could not be made, the request gave
/// up waiting for it, or the task carrying it did not return.
///
/// It lives HERE, beside the seam and the budget, so every control plane that
/// snapshots a warehouse through this engine reports the same three outcomes
/// rather than keeping its own copy of them.
#[derive(Debug)]
pub enum CopyFailure {
    /// The databases could not be copied.
    Unavailable(WarehouseUnavailable),
    /// The request waited [`COPY_REQUEST_BUDGET`] and gave up, so the copy is
    /// abandoned rather than reported as done.
    GaveUp(Duration),
    /// The blocking task carrying the copy did not return a result.
    Lost,
}

impl CopyFailure {
    /// True when the failure is this emulator being at its capacity for open
    /// databases — the one of these a control plane reports as a quota error
    /// rather than as a server fault.
    pub fn is_capacity(&self) -> bool {
        matches!(self, Self::Unavailable(WarehouseUnavailable::TooManyOpen(_)))
    }
}

impl std::fmt::Display for CopyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(unavailable) => write!(f, "{unavailable}"),
            Self::GaveUp(budget) => write!(
                f,
                "the copy did not finish within {} seconds and was abandoned",
                budget.as_secs()
            ),
            Self::Lost => write!(f, "the copy did not return a result"),
        }
    }
}

impl std::error::Error for CopyFailure {}

/// [`capture_warehouse`] run the way a REQUEST must run it: on the blocking pool,
/// under [`COPY_REQUEST_BUDGET`].
///
/// Copying databases drives the SQL engine, so it may never run on the async
/// worker the request is served on, and the wait for it has to be bounded — the
/// budget is longer than the deadline the copy holds itself to, so what ends a
/// copy is its own deadline and not the request's patience. Call it with NO state
/// guard held, so one caller's copy serialises nothing else.
pub async fn capture_warehouse_bounded<Eng: Send + Sync + 'static>(
    engine: &std::sync::Arc<Eng>,
    targets: Vec<ConnectionTarget>,
) -> Result<WarehouseCapture, CopyFailure> {
    let engine = std::sync::Arc::clone(engine);
    collect(
        tokio::time::timeout(
            COPY_REQUEST_BUDGET,
            tokio::task::spawn_blocking(move || capture_warehouse(&*engine, &targets)),
        )
        .await,
    )
}

/// [`restore_warehouse`] run the way a REQUEST must run it: bounded and off the
/// async worker exactly as [`capture_warehouse_bounded`] is, and for the same
/// reasons.
pub async fn restore_warehouse_bounded<Eng: Send + Sync + 'static>(
    engine: &std::sync::Arc<Eng>,
    capture: std::sync::Arc<WarehouseCapture>,
    target: ConnectionTarget,
) -> Result<usize, CopyFailure> {
    let engine = std::sync::Arc::clone(engine);
    collect(
        tokio::time::timeout(
            COPY_REQUEST_BUDGET,
            tokio::task::spawn_blocking(move || restore_warehouse(&*engine, &capture, &target)),
        )
        .await,
    )
}

/// The outcome of one bounded blocking copy: the copy's own answer, or why there
/// is none.
type JoinedCopy<T> = Result<
    Result<Result<T, WarehouseUnavailable>, tokio::task::JoinError>,
    tokio::time::error::Elapsed,
>;

/// Read one bounded copy's three failure modes off the join result.
fn collect<T>(joined: JoinedCopy<T>) -> Result<T, CopyFailure> {
    match joined {
        Ok(Ok(Ok(done))) => Ok(done),
        Ok(Ok(Err(unavailable))) => Err(CopyFailure::Unavailable(unavailable)),
        Ok(Err(_panicked)) => Err(CopyFailure::Lost),
        Err(_elapsed) => Err(CopyFailure::GaveUp(COPY_REQUEST_BUDGET)),
    }
}

/// Lock a database, giving up at `deadline` rather than waiting for a statement
/// that may be running on it for as long as [`MAX_ENGINE_DEADLINE`] allows.
///
/// Polls rather than blocking: a `Mutex` has no timed acquire, and a copy must have
/// a point at which it gives up. The sleeps run on a blocking-pool thread.
fn lock_database_until(
    handle: &DatabaseHandle,
    deadline: Instant,
) -> Option<std::sync::MutexGuard<'_, Database>> {
    loop {
        match handle.try_lock() {
            Ok(guard) => return Some(guard),
            // A database whose statement panicked is still a database: the poisoning
            // says nothing about the pages the engine committed.
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

/// Copy `source` over `destination` with the engine's own page-level copy, and
/// report how many pages it moved.
///
/// `sqlite3_backup_*` between two OPEN connections, so no path is named and no
/// file is opened — unlike `VACUUM INTO`, which the authorizer denies. It also
/// runs beneath SQL, so it copies every table, index, view and trigger of the
/// source exactly, with no dialect or quoting of its own to get wrong.
///
/// Driven one step at a time rather than through `run_to_completion`, so `deadline`
/// is checked between steps: that is the cancellation point a bounded request
/// needs. `max_page_count` is a property of the destination CONNECTION and not of
/// the pages copied into it, so the destination keeps the bound [`Database::open`]
/// installed and the copy cannot exceed it — the engine answers `SQLITE_FULL` if a
/// source ever did.
fn copy_database(
    source: &Database,
    destination: &mut Database,
    deadline: Instant,
) -> Result<i64, WarehouseUnavailable> {
    let engine_error =
        |error: rusqlite::Error| WarehouseUnavailable::Engine(scrub_paths(&error.to_string()));
    let backup = rusqlite::backup::Backup::new(&source.connection, &mut destination.connection)
        .map_err(engine_error)?;
    loop {
        if Instant::now() >= deadline {
            return Err(WarehouseUnavailable::TimedOut(COPY_TIMEOUT));
        }
        match backup.step(COPY_PAGES_PER_STEP).map_err(engine_error)? {
            rusqlite::backup::StepResult::Done => break,
            rusqlite::backup::StepResult::More => {}
            // Neither end is shared with anything but this copy, which holds both
            // locks, so a busy or locked step is the engine reporting a state this
            // copy cannot resolve by waiting.
            busy => {
                return Err(WarehouseUnavailable::Engine(format!(
                    "the engine answered {busy:?} while copying a database"
                )))
            }
        }
    }
    // The schemas the source holds are the source's catalogue, and the copy is
    // the same database: a captured warehouse restores the schemas its tables
    // are held under, not only the tables.
    destination.schemas = source.schemas.clone();
    Ok(i64::from(backup.progress().pagecount.max(0)))
}
