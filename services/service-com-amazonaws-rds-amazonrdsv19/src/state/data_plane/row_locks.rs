// SPDX-License-Identifier: CC0-1.0

//! The ROW LOCKS the transactions open on one engine endpoint hold, and the refusals a
//! client reads off them.
//!
//! The table itself is the tree's ONE lock table
//! ([`::service_common::sql_row_locks`]), re-exported here under the name this module tree
//! reaches it by: a DB cluster's data is addressed through more than one surface — the engine
//! endpoints served here and the Data API HTTP endpoint another service serves — so WHO HOLDS
//! WHAT cannot be a table of this crate's own, or a row one surface's transaction holds would
//! be a row the other's walks straight over.
//!
//! What stays here is which TABLE a given endpoint uses ([`super::sql_engine::InstanceDatabase`]):
//! a DB cluster's is the one table of that cluster's volume, shared with its other surfaces,
//! and a DB instance's is its own. Which rows a statement locks is [`super::lock_plan`]'s
//! reading of the statement, and the session ids the holders are named by are the
//! `connection_id` of [`super::wire_session::SessionFacts`] — a PostgreSQL backend pid, a
//! MySQL connection id.
//!
//! ... and HOW EACH ENGINE WORDS the conditions that table answers with. One lock table
//! answers one condition — the row is held and the statement would not wait, the wait ran
//! out, waiting would close a cycle — and each engine states that condition in ITS OWN words,
//! with its own error number and SQLSTATE: the sentence and the code are what a client
//! branches, retries and logs on, so a MySQL session reads MySQL's and a PostgreSQL session
//! reads PostgreSQL's. Every sentence below is quoted from the engine's own reference (MySQL's
//! "Server Error Message Reference", PostgreSQL's messages for the same conditions), and the
//! fixed ones carry no name or value of the statement that met them, because those messages
//! have no place for one.

use std::time::Duration;

pub use ::service_common::sql_row_locks::{
    LockMode, LockOutcome, LockTable, RowKey, WaitPolicy, MAX_HELD_ROWS,
};

use super::sql_engine::{EngineError, EngineErrorKind};
use super::wire_session::SessionFacts;
use super::EngineWire;

/// MySQL's own default for `innodb_lock_wait_timeout`, in seconds: "the length of time in
/// seconds an InnoDB transaction waits for a row lock before giving up", whose default value
/// is 50. A MySQL session therefore ALWAYS has a finite wait for a row, which is why a wait
/// this endpoint ends is that engine's lock-wait timeout.
const MYSQL_DEFAULT_LOCK_WAIT_SECONDS: u64 = 50;

/// How long a statement of this session may WAIT for a row another transaction holds, or
/// `None` where the session has asked for no limit of its own and waits for as long as the
/// statement itself may run.
///
/// Each engine's own parameter, read as that engine defines it:
///
/// * MySQL's `innodb_lock_wait_timeout`, in SECONDS, which every session has — its default is
///   [`MYSQL_DEFAULT_LOCK_WAIT_SECONDS`];
/// * PostgreSQL's `lock_timeout`, in MILLISECONDS, where "a value of zero (the default)
///   disables the timeout", so a session that sets none waits as long as its statement may.
#[must_use]
pub fn lock_wait_limit(facts: &SessionFacts) -> Option<Duration> {
    match facts.wire {
        EngineWire::MySql => Some(Duration::from_secs(
            facts
                .setting("innodb_lock_wait_timeout")
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(MYSQL_DEFAULT_LOCK_WAIT_SECONDS),
        )),
        EngineWire::Postgres => facts
            .setting("lock_timeout")
            .and_then(|value| value.trim().parse().ok())
            .filter(|milliseconds| *milliseconds > 0)
            .map(Duration::from_millis),
    }
}

/// What a session on `wire` is told when a row of `table` it asked for WITHOUT waiting is held
/// by another transaction.
///
/// MySQL's `ER_LOCK_NOWAIT` is wholly fixed text — it names no table, row or value — while
/// PostgreSQL's `lock_not_available` names the relation the row belongs to.
#[must_use]
pub fn held_by_another_transaction(wire: EngineWire, table: &str) -> EngineError {
    let message = match wire {
        EngineWire::MySql => {
            "Statement aborted because lock(s) could not be acquired immediately and NOWAIT \
             is set."
                .to_string()
        }
        EngineWire::Postgres => {
            format!("could not obtain lock on row in relation \"{table}\"")
        }
    };
    EngineError::new(EngineErrorKind::LockNotAvailable, message)
}

/// And what it is told when its WAIT for such a row ran out: each engine's own sentence for
/// the lock-wait timeout, both of them fixed text.
#[must_use]
pub fn the_wait_ran_out(wire: EngineWire) -> EngineError {
    let message = match wire {
        EngineWire::MySql => "Lock wait timeout exceeded; try restarting transaction",
        EngineWire::Postgres => "canceling statement due to lock timeout",
    };
    EngineError::new(EngineErrorKind::LockWaitTimeout, message)
}

/// And what the one transaction of a waiting CYCLE the engine breaks is told.
#[must_use]
pub fn the_cycle_was_broken(wire: EngineWire) -> EngineError {
    let message = match wire {
        EngineWire::MySql => "Deadlock found when trying to get lock; try restarting transaction",
        EngineWire::Postgres => "deadlock detected",
    };
    EngineError::new(EngineErrorKind::Deadlock, message)
}
