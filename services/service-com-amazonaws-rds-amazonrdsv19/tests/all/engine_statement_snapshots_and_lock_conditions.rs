// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN a transaction's statements read, and for the refusals a client
//! reads off a row another transaction holds.
//!
//! The rules under test, each asserted over a table of inputs spanning what it generalises
//! over:
//!
//! 1. a session whose `autocommit` is off is INSIDE A TRANSACTION from its first statement,
//!    without having sent one that opens one: what it writes is invisible to every other
//!    reader of that database — another session on the engine's wire and the cluster's other
//!    door, the RDS Data API — until it commits, and a rollback leaves no trace of it on
//!    either door;
//! 2. the ISOLATION LEVEL decides when a transaction's statements read: at READ COMMITTED a
//!    statement sees what was committed before IT ran, so a statement after another session's
//!    commit reads that commit, while REPEATABLE READ and SERIALIZABLE hold every statement
//!    of the transaction to the rows it started with;
//! 3. a row another transaction holds is refused in the words of the engine THE SESSION IS
//!    TALKING TO: its own sentence, its own error number and its own SQLSTATE, for a
//!    statement that would not wait (MySQL `ER_LOCK_NOWAIT`, PostgreSQL `lock_not_available`)
//!    and for one whose wait for the row ran out (MySQL `ER_LOCK_WAIT_TIMEOUT`, PostgreSQL's
//!    lock timeout) — and a statement whose wait ran out wrote nothing.
//!
//! Every surface is driven through its own front door: the control plane as awsQuery calls
//! through the router, the engine endpoints as clients speaking the engine's own protocol on
//! the published address ([`crate::db_instance_engine_endpoint`]), and the Data API as the
//! restJson1 calls its own cross-surface suite sends
//! ([`crate::cluster_data_api_and_engine_surfaces`], whose cluster harness this suite
//! provisions its Aurora MySQL cluster with). Everything runs in ONE process.

use crate::cluster_data_api_and_engine_surfaces::{
    available_cluster, data_api_rows, emulator as cluster_emulator, execute, Cluster, Row, Wire,
};
use crate::db_instance_engine_endpoint::{
    available_instance, mysql_login, wire_login, MySqlSession, Refusal, Rows, Session, Transport,
    Wire as EndpointWire, DATABASE, MASTER_PASSWORD, MASTER_USER,
};
use crate::db_instances::emulator;

/// The Aurora MySQL cluster rule 1 is asserted on: both of its doors address one database, so
/// what one door has not committed is what the other must not read.
const SHARED_CLUSTER: Row = Row {
    case: "an Aurora MySQL cluster both doors address",
    identifier: "emu-implicit-tx-mysql",
    engine: "aurora-mysql",
    master_username: "myadmin",
    database: "shopdb",
    wire: Wire::MySql,
    catalog_query: "SELECT count(*) FROM information_schema.SCHEMATA",
};

/// The table rule 1 is asserted over, and the one row committed into it before any session
/// opens a transaction.
const CREATE_SHARED_TABLE: &str =
    "CREATE TABLE t (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL) ENGINE=InnoDB";
const SEED_SHARED_TABLE: &str = "INSERT INTO t (id, label) VALUES (1, 'committed')";

/// How both doors read that table back.
const SELECT_SHARED_TABLE: &str = "SELECT id, label FROM t ORDER BY id";

/// What one uncommitted statement of a session with `autocommit` off must show each reader.
struct Uncommitted {
    case: &'static str,
    /// The statement the session runs without committing.
    statement: &'static str,
    /// What that session's own next read answers, as `(id, label)` pairs.
    inside: &'static [(i64, &'static str)],
    /// `COMMIT` or `ROLLBACK`.
    ending: &'static str,
    /// What every reader answers once the session has ended its transaction that way.
    after: &'static [(i64, &'static str)],
}

/// The rows span the statements a transaction can hold uncommitted — an insert, an update and
/// a delete — and both ways of ending it. Each case leaves the table as it found it, so the
/// rows after it read the same committed baseline.
const UNCOMMITTED: &[Uncommitted] = &[
    Uncommitted {
        case: "an insert that rolls back",
        statement: "INSERT INTO t (id, label) VALUES (3, 'pending')",
        inside: &[(1, "committed"), (3, "pending")],
        ending: "ROLLBACK",
        after: &[(1, "committed")],
    },
    Uncommitted {
        case: "an update that rolls back",
        statement: "UPDATE t SET label = 'moved' WHERE id = 1",
        inside: &[(1, "moved")],
        ending: "ROLLBACK",
        after: &[(1, "committed")],
    },
    Uncommitted {
        case: "a delete that rolls back",
        statement: "DELETE FROM t WHERE id = 1",
        inside: &[],
        ending: "ROLLBACK",
        after: &[(1, "committed")],
    },
    Uncommitted {
        case: "an insert that commits",
        statement: "INSERT INTO t (id, label) VALUES (4, 'kept')",
        inside: &[(1, "committed"), (4, "kept")],
        ending: "COMMIT",
        after: &[(1, "committed"), (4, "kept")],
    },
    Uncommitted {
        case: "a delete that commits, putting the baseline back",
        statement: "DELETE FROM t WHERE id = 4",
        inside: &[(1, "committed")],
        ending: "COMMIT",
        after: &[(1, "committed")],
    },
];

/// The expected pairs as the comparison holds them.
fn pairs(rows: &[(i64, &'static str)]) -> Vec<(i64, String)> {
    rows.iter()
        .map(|(id, label)| (*id, (*label).to_string()))
        .collect()
}

/// The `(id, label)` pairs a two-column result set off either engine's wire carries: both
/// wires report the values as text, which is what the comparison is made on.
fn engine_pairs(rows: &Rows) -> Vec<(i64, String)> {
    rows.rows
        .iter()
        .map(|row| {
            (
                row.first()
                    .cloned()
                    .flatten()
                    .unwrap_or_default()
                    .parse()
                    .unwrap_or_default(),
                row.get(1).cloned().flatten().unwrap_or_default(),
            )
        })
        .collect()
}

/// One session on the cluster's engine endpoint, as the cluster's master user.
async fn cluster_session(case: &str, row: &Row, cluster: &Cluster) -> MySqlSession {
    mysql_login(
        &cluster.address,
        cluster.port,
        Transport::Plaintext,
        row.master_username,
        &cluster.password,
        Some(row.database),
    )
    .await
    .unwrap_or_else(|refusal| {
        panic!("case {case:?}: the master user must be admitted, got {refusal:?}")
    })
}

/// Run one statement that must be served.
async fn run(case: &str, session: &mut MySqlSession, sql: &str) -> Rows {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_a_session_with_autocommit_off_has_not_committed_is_read_by_no_other_door() {
    let router = cluster_emulator();
    let row = &SHARED_CLUSTER;
    let cluster = available_cluster(&router, row).await;
    let database = row.database;
    execute(&router, row.case, &cluster, database, CREATE_SHARED_TABLE, None).await;
    execute(&router, row.case, &cluster, database, SEED_SHARED_TABLE, None).await;

    // The session a driver's `autocommit=False` opens: it turns autocommit off and sends no
    // statement that opens a transaction.
    let mut writer = cluster_session("the writer", row, &cluster).await;
    run("the writer", &mut writer, "SET autocommit = 0").await;
    let mut reader = cluster_session("the other session", row, &cluster).await;

    // What is COMMITTED at the start of each case, which every other reader must see while
    // the writer holds its own work: the seeded row, and then whatever the case before left.
    let mut committed = pairs(&[(1, "committed")]);
    for case in UNCOMMITTED {
        let Uncommitted {
            case,
            statement,
            inside,
            ending,
            after,
        } = case;
        run(case, &mut writer, statement).await;

        // Its own next read sees what it wrote: the statement ran, inside the transaction it
        // is in.
        let own = engine_pairs(&run(case, &mut writer, SELECT_SHARED_TABLE).await);
        assert_eq!(
            own,
            pairs(inside),
            "case {case:?}: the writing session must read its own uncommitted write, expected \
             {inside:?}, got {own:?}"
        );

        // Neither door reads it: another session on the engine's wire, and the Data API's
        // door onto the same database.
        let natively = engine_pairs(&run(case, &mut reader, SELECT_SHARED_TABLE).await);
        assert_eq!(
            natively, committed,
            "case {case:?}: another session must read only the committed rows, expected \
             {committed:?}, got {natively:?}"
        );
        let through_the_api = data_api_rows(
            &execute(
                &router,
                case,
                &cluster,
                database,
                SELECT_SHARED_TABLE,
                None,
            )
            .await,
        );
        assert_eq!(
            through_the_api, committed,
            "case {case:?}: the Data API must read only the committed rows, expected \
             {committed:?}, got {through_the_api:?}"
        );

        // And once the session ends its transaction, both doors read exactly what it left.
        run(case, &mut writer, ending).await;
        let natively = engine_pairs(&run(case, &mut reader, SELECT_SHARED_TABLE).await);
        assert_eq!(
            natively,
            pairs(after),
            "case {case:?}: after the {ending}, another session must read {after:?}, got \
             {natively:?}"
        );
        let through_the_api = data_api_rows(
            &execute(
                &router,
                case,
                &cluster,
                database,
                SELECT_SHARED_TABLE,
                None,
            )
            .await,
        );
        assert_eq!(
            through_the_api,
            pairs(after),
            "case {case:?}: after the {ending}, the Data API must read {after:?}, got \
             {through_the_api:?}"
        );
        committed = pairs(after);
    }
}

/// What one isolation level shows a transaction whose neighbour commits while it is open.
struct Level {
    case: &'static str,
    wire: EndpointWire,
    engine: &'static str,
    version: &'static str,
    identifier: &'static str,
    /// The statements that put the session's next transaction at this level, in order, and
    /// the one that opens it.
    opening: &'static [&'static str],
    /// Whether a statement of that transaction, run after the neighbour's commit, reads it.
    sees_the_commit: bool,
}

/// The rows span both engines' wires and the levels each one lets a client ask for: the two
/// that hold a transaction to one snapshot and the one whose every statement reads afresh.
/// PostgreSQL takes the level on the statement that opens the transaction; MySQL takes it as
/// a session variable, which is the spelling each engine's own documentation gives.
const LEVELS: &[Level] = &[
    Level {
        case: "a PostgreSQL session at read committed, its own default",
        wire: EndpointWire::Postgres,
        engine: "postgres",
        version: "16.4",
        identifier: "emu-snapshot-pg-rc",
        opening: &["BEGIN"],
        sees_the_commit: true,
    },
    Level {
        case: "a PostgreSQL session at repeatable read",
        wire: EndpointWire::Postgres,
        engine: "postgres",
        version: "16.4",
        identifier: "emu-snapshot-pg-rr",
        opening: &["BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ"],
        sees_the_commit: false,
    },
    Level {
        case: "a PostgreSQL session at serializable",
        wire: EndpointWire::Postgres,
        engine: "postgres",
        version: "16.4",
        identifier: "emu-snapshot-pg-ser",
        opening: &["BEGIN TRANSACTION ISOLATION LEVEL SERIALIZABLE"],
        sees_the_commit: false,
    },
    Level {
        case: "a MySQL session at repeatable read, its own default",
        wire: EndpointWire::MySql,
        engine: "mysql",
        version: "8.0.39",
        identifier: "emu-snapshot-my-rr",
        opening: &["BEGIN"],
        sees_the_commit: false,
    },
    Level {
        case: "a MySQL session at read committed",
        wire: EndpointWire::MySql,
        engine: "mysql",
        version: "8.0.39",
        identifier: "emu-snapshot-my-rc",
        opening: &[
            "SET SESSION transaction_isolation = 'READ-COMMITTED'",
            "BEGIN",
        ],
        sees_the_commit: true,
    },
];

/// The table every level's pair of sessions reads and writes, declared in the form both
/// engines take.
const CREATE_VERSIONS: &str = "CREATE TABLE v (id INT PRIMARY KEY, v VARCHAR(32) NOT NULL)";
const SEED_VERSIONS: &str = "INSERT INTO v (id, v) VALUES (1, 'old')";
const SELECT_VERSIONS: &str = "SELECT id, v FROM v ORDER BY id";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_a_transactions_statement_reads_is_the_level_it_runs_at() {
    let router = emulator();
    for level in LEVELS {
        let case = level.case;
        let (address, port) = available_instance(
            &router,
            level.identifier,
            level.engine,
            &[("EngineVersion", level.version.to_string())],
        )
        .await;
        let mut writer = wire_login(case, level.wire, &address, port, DATABASE).await;
        let mut reader = wire_login(case, level.wire, &address, port, DATABASE).await;
        writer.run(case, CREATE_VERSIONS).await;
        writer.run(case, SEED_VERSIONS).await;

        // The reader opens its transaction at this level and reads the committed row.
        for opening in level.opening {
            reader.run(case, opening).await;
        }
        let first = engine_pairs(&reader.run(case, SELECT_VERSIONS).await);
        assert_eq!(
            first,
            vec![(1, "old".to_string())],
            "case {case:?}: the transaction must read the committed row, got {first:?}"
        );

        // Its neighbour changes a row and inserts another, in a transaction of its own, and
        // commits.
        writer.run(case, "BEGIN").await;
        writer
            .run(case, "UPDATE v SET v = 'new' WHERE id = 1")
            .await;
        writer
            .run(case, "INSERT INTO v (id, v) VALUES (2, 'added')")
            .await;
        writer.run(case, "COMMIT").await;

        // What the reader's NEXT statement, inside the transaction it still holds, reads.
        let again = engine_pairs(&reader.run(case, SELECT_VERSIONS).await);
        let expected: Vec<(i64, String)> = if level.sees_the_commit {
            vec![(1, "new".to_string()), (2, "added".to_string())]
        } else {
            vec![(1, "old".to_string())]
        };
        assert_eq!(
            again, expected,
            "case {case:?}: the next statement of the open transaction must read {expected:?}, \
             got {again:?}"
        );
        // Its transaction is ended with a ROLLBACK: it wrote nothing, and a transaction at
        // the SERIALIZABLE level that read a relation another transaction has since written
        // is one the engine cannot order, so committing it is a refusal of its own — the
        // subject of a sibling suite ([`crate::engine_session_isolation`]) and not of this
        // one, which is about WHEN its statements read.
        reader.run(case, "ROLLBACK").await;

        // And once that transaction has ended, the commit is there for it too: no level hides
        // a committed row from a transaction that started afterwards.
        let afterwards = engine_pairs(&reader.run(case, SELECT_VERSIONS).await);
        assert_eq!(
            afterwards,
            vec![(1, "new".to_string()), (2, "added".to_string())],
            "case {case:?}: once the transaction has ended, its session must read the commit, \
             got {afterwards:?}"
        );
    }
}

/// What the engine of one wire answers a statement that cannot have a row another transaction
/// holds.
struct Conditions {
    case: &'static str,
    wire: EndpointWire,
    engine: &'static str,
    version: &'static str,
    identifier: &'static str,
    /// The statement that bounds this session's wait for a row, in that engine's own
    /// parameter and units.
    bound_the_wait: &'static str,
    /// The code and the sentence a statement that would NOT wait is refused with: MySQL's
    /// error number, PostgreSQL's SQLSTATE.
    would_not_wait: (&'static str, &'static str),
    /// ... and the ones a statement whose WAIT for the row ran out is refused with.
    waited: (&'static str, &'static str),
}

/// The rows span the two engines this emulator serves a wire for. Every sentence is the one
/// its engine's own reference gives for that condition, and both of MySQL's are fixed text:
/// they name no table, row or value.
const CONDITIONS: &[Conditions] = &[
    Conditions {
        case: "a MySQL session",
        wire: EndpointWire::MySql,
        engine: "mysql",
        version: "8.0.39",
        identifier: "emu-lockwords-mysql",
        // "The length of time in seconds an InnoDB transaction waits for a row lock before
        // giving up."
        bound_the_wait: "SET SESSION innodb_lock_wait_timeout = 1",
        would_not_wait: (
            "3572",
            "Statement aborted because lock(s) could not be acquired immediately and NOWAIT \
             is set.",
        ),
        waited: (
            "1205",
            "Lock wait timeout exceeded; try restarting transaction",
        ),
    },
    Conditions {
        case: "a PostgreSQL session",
        wire: EndpointWire::Postgres,
        engine: "postgres",
        version: "16.4",
        identifier: "emu-lockwords-postgres",
        // "Abort any statement that waits longer than the specified amount of time while
        // attempting to acquire a lock ... in milliseconds".
        bound_the_wait: "SET lock_timeout = 250",
        would_not_wait: (
            "55P03",
            "could not obtain lock on row in relation \"seats\"",
        ),
        waited: ("55P03", "canceling statement due to lock timeout"),
    },
];

/// The table the lock conditions are asserted over: two rows, one of which a transaction
/// holds while the other stays free.
const CREATE_SEATS: &str = "CREATE TABLE seats (id INT PRIMARY KEY, holder VARCHAR(16) NOT NULL)";
const SEED_SEATS: &str = "INSERT INTO seats (id, holder) VALUES (1, 'first'), (2, 'second')";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_row_another_transaction_holds_is_refused_in_the_words_of_the_sessions_own_engine() {
    let router = emulator();
    for conditions in CONDITIONS {
        let case = conditions.case;
        let (address, port) = available_instance(
            &router,
            conditions.identifier,
            conditions.engine,
            &[("EngineVersion", conditions.version.to_string())],
        )
        .await;
        let mut holder = wire_login(case, conditions.wire, &address, port, DATABASE).await;
        let mut waiter = wire_login(case, conditions.wire, &address, port, DATABASE).await;
        holder.run(case, CREATE_SEATS).await;
        holder.run(case, SEED_SEATS).await;

        // One transaction holds row 1 and nothing else.
        holder.run(case, "BEGIN").await;
        holder
            .run(case, "UPDATE seats SET holder = 'holder' WHERE id = 1")
            .await;

        // A statement that asks for the free row with the same clause is served: the hold is
        // on the ROW, so the refusals below are about row 1 and not about the clause.
        let free = waiter
            .query("SELECT holder FROM seats WHERE id = 2 FOR UPDATE NOWAIT")
            .await;
        assert!(
            free.is_ok(),
            "case {case:?}: a row no transaction holds must be lockable, got {free:?}"
        );

        // A statement that will not wait for the held row: refused with the code and the
        // sentence this engine words that condition with.
        let refusal = refused(
            case,
            &mut waiter,
            "SELECT holder FROM seats WHERE id = 1 FOR UPDATE NOWAIT",
        )
        .await;
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            conditions.would_not_wait,
            "case {case:?}: expected the engine's own refusal for a row it would not wait for, \
             got {refusal:?}"
        );

        // And one that WAITS, with its wait bounded by this engine's own parameter: refused
        // with that engine's lock-wait condition once the wait runs out.
        waiter.run(case, conditions.bound_the_wait).await;
        let refusal = refused(
            case,
            &mut waiter,
            "UPDATE seats SET holder = 'waiter' WHERE id = 1",
        )
        .await;
        assert_eq!(
            (refusal.code.as_str(), refusal.message.as_str()),
            conditions.waited,
            "case {case:?}: expected the engine's own refusal for a wait that ran out, got \
             {refusal:?}"
        );

        // The refused statement wrote nothing: the row still holds what the transaction
        // holding it put there, and after its rollback what it held before.
        holder.run(case, "ROLLBACK").await;
        let seated = holder
            .value(case, "SELECT holder FROM seats WHERE id = 1")
            .await;
        assert_eq!(
            seated, "first",
            "case {case:?}: a statement whose wait ran out must write nothing, got {seated:?}"
        );
    }
}

/// Run one statement that must be REFUSED, and answer with the refusal.
async fn refused(case: &str, session: &mut Session, sql: &str) -> Refusal {
    match session.query(sql).await {
        Err(refusal) => refusal,
        Ok(rows) => panic!("case {case:?}: {sql:?} must be refused, it answered {rows:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_reads_back_the_wait_it_bounded_for_itself() {
    let router = emulator();
    let case = "the wait a MySQL session bounded";
    let (address, port) = available_instance(
        &router,
        "emu-lockwait-readback",
        "mysql",
        &[("EngineVersion", "8.0.39".to_string())],
    )
    .await;
    let mut session = mysql_login(
        &address,
        port,
        Transport::Plaintext,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| panic!("case {case:?}: the master user must be admitted, got {refusal:?}"));
    // The value a client sets for itself is the value it reads back, which is what makes the
    // bound the statement above waits under this session's own.
    for value in ["1", "7"] {
        session
            .query(&format!("SET SESSION innodb_lock_wait_timeout = {value}"))
            .await
            .unwrap_or_else(|refusal| panic!("case {case:?}: the SET must run, got {refusal:?}"));
        let read_back = session
            .query("SELECT @@innodb_lock_wait_timeout")
            .await
            .unwrap_or_else(|refusal| {
                panic!("case {case:?}: the read-back must run, got {refusal:?}")
            });
        assert_eq!(
            read_back.only(),
            Some(value),
            "case {case:?}: expected the session's own {value:?}, got {read_back:?}"
        );
    }
}
