// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what one SESSION's transaction shows every other session on the
//! same DB instance endpoint.
//!
//! The rules under test:
//!
//! * a row a transaction has written and not committed is invisible to every other session
//!   until it commits: another session reads the rows as they stood before;
//! * a transaction at REPEATABLE READ reads the same rows throughout: a value another session
//!   commits while it is open does not move under it — while at READ COMMITTED, the level a
//!   PostgreSQL session runs at by default, its next statement reads that commit, which is
//!   what the level means and what the sibling suite
//!   ([`crate::engine_statement_snapshots_and_lock_conditions`]) asserts over every level and
//!   both wires;
//! * once a transaction commits, every session that reads afterwards reads what it wrote,
//!   and a `ROLLBACK` leaves the shared database exactly as the transaction found it;
//! * two transactions that write DIFFERENT rows both commit, and each row ends up holding
//!   the value the transaction that wrote it chose.
//!
//! The table spans what the rules generalise over: an insert, an update and a delete; a
//! transaction that commits and one that rolls back; a reader that is itself inside a
//! transaction and one that is not; and the concurrent pair whose writes do not overlap.
//!
//! Every client here is the wire client the endpoint suite drives
//! ([`crate::db_instance_engine_endpoint`]), so each "session" is a real connection with its
//! own transaction, in one process.

use crate::db_instance_engine_endpoint::{
    available_instance, postgres_login, PostgresSession, Transport,
};
use crate::db_instances::emulator;

/// The master user the instances here are created with, and its test password — the only
/// place either appears.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";
/// The `DBName` the instances here are created with.
const DATABASE: &str = "appdb";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// Open a session as the master user on a PostgreSQL instance's published endpoint.
async fn session(address: &str, port: u16) -> PostgresSession {
    postgres_login(
        address,
        port,
        Transport::Tls,
        MASTER_USER,
        MASTER_PASSWORD,
        Some(DATABASE),
    )
    .await
    .expect("the master user must be admitted")
}

/// Run one statement, panicking with the case it belongs to where the server refused it.
async fn run(case: &str, session: &mut PostgresSession, sql: &str) {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"));
}

/// The one column of every row a statement answered with.
async fn column(case: &str, session: &mut PostgresSession, sql: &str) -> Vec<String> {
    session
        .query(sql)
        .await
        .unwrap_or_else(|refusal| panic!("case {case:?}: {sql:?} must run, got {refusal:?}"))
        .rows
        .iter()
        .map(|row| {
            row.first()
                .and_then(Option::as_deref)
                .unwrap_or("<null>")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn an_uncommitted_write_is_invisible_until_the_transaction_commits() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-isolation-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut writer = session(&address, port).await;
    let mut reader = session(&address, port).await;
    run(
        "setup",
        &mut writer,
        "CREATE TABLE ledger (id integer PRIMARY KEY, note text)",
    )
    .await;
    run(
        "setup",
        &mut writer,
        "INSERT INTO ledger VALUES (1, 'first'), (2, 'second')",
    )
    .await;

    // Each row is one uncommitted statement: what the writer's own session sees, what
    // another session sees while the transaction is open, and what it sees after the end
    // the row names. The rows span an insert, an update and a delete, committed and rolled
    // back.
    struct Case {
        case: &'static str,
        /// The statement the open transaction runs.
        statement: &'static str,
        /// What the writer's own session reads back inside its transaction.
        inside: &'static [&'static str],
        /// What another session reads while that transaction is open.
        outside: &'static [&'static str],
        /// `COMMIT` or `ROLLBACK`.
        ending: &'static str,
        /// What every session reads after the ending.
        after: &'static [&'static str],
    }
    let select = "SELECT note FROM ledger ORDER BY id";
    for Case {
        case,
        statement,
        inside,
        outside,
        ending,
        after,
    } in [
        Case {
            case: "an insert that commits",
            statement: "INSERT INTO ledger VALUES (3, 'third')",
            inside: &["first", "second", "third"],
            outside: &["first", "second"],
            ending: "COMMIT",
            after: &["first", "second", "third"],
        },
        Case {
            case: "an update that commits",
            statement: "UPDATE ledger SET note = 'changed' WHERE id = 1",
            inside: &["changed", "second", "third"],
            outside: &["first", "second", "third"],
            ending: "COMMIT",
            after: &["changed", "second", "third"],
        },
        Case {
            case: "a delete that rolls back",
            statement: "DELETE FROM ledger WHERE id = 3",
            inside: &["changed", "second"],
            outside: &["changed", "second", "third"],
            ending: "ROLLBACK",
            after: &["changed", "second", "third"],
        },
        Case {
            case: "an insert that rolls back",
            statement: "INSERT INTO ledger VALUES (4, 'fourth')",
            inside: &["changed", "second", "third", "fourth"],
            outside: &["changed", "second", "third"],
            ending: "ROLLBACK",
            after: &["changed", "second", "third"],
        },
    ] {
        run(case, &mut writer, "BEGIN").await;
        run(case, &mut writer, statement).await;
        let seen_inside = column(case, &mut writer, select).await;
        assert_eq!(
            seen_inside, inside,
            "case {case:?}: the writing transaction must read its own write, expected \
             {inside:?}, got {seen_inside:?}"
        );
        let seen_outside = column(case, &mut reader, select).await;
        assert_eq!(
            seen_outside, outside,
            "case {case:?}: another session must not read an uncommitted row, expected \
             {outside:?}, got {seen_outside:?}"
        );
        run(case, &mut writer, ending).await;
        // A session opened after the ending: the committed rows are durable and the rolled
        // back ones were never there.
        let mut fresh = session(&address, port).await;
        for (whose, reading) in [
            ("the writer", &mut writer),
            ("the other session", &mut reader),
            ("a session opened after it", &mut fresh),
        ] {
            let seen = column(case, reading, select).await;
            assert_eq!(
                seen, after,
                "case {case:?}: after the {ending}, {whose} must read {after:?}, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_transaction_reads_the_rows_as_they_stood_when_it_began() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-isolation-snapshot-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut reader = session(&address, port).await;
    let mut writer = session(&address, port).await;
    let case = "a stable snapshot";
    run(
        case,
        &mut writer,
        "CREATE TABLE notes (id integer PRIMARY KEY, body text)",
    )
    .await;
    run(case, &mut writer, "INSERT INTO notes VALUES (1, 'first')").await;

    let select = "SELECT body FROM notes WHERE id = 1";
    // The level that reads ONE snapshot: "all statements of the current transaction can only
    // see rows committed before the first query ... of this transaction". A transaction at the
    // session's default level (READ COMMITTED) reads each statement's own snapshot instead,
    // which the sibling suite asserts.
    run(
        case,
        &mut reader,
        "BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ",
    )
    .await;
    let first = column(case, &mut reader, select).await;
    assert_eq!(
        first,
        vec!["first".to_string()],
        "case {case:?}: the transaction must read the committed row, got {first:?}"
    );
    // Another session commits a new value while that transaction is open.
    run(case, &mut writer, "BEGIN").await;
    run(
        case,
        &mut writer,
        "UPDATE notes SET body = 'second' WHERE id = 1",
    )
    .await;
    run(case, &mut writer, "COMMIT").await;

    let again = column(case, &mut reader, select).await;
    assert_eq!(
        again,
        vec!["first".to_string()],
        "case {case:?}: the open transaction must keep reading the row as it stood when it \
         began, got {again:?}"
    );
    run(case, &mut reader, "COMMIT").await;
    // The commit the other session made is not lost: every session reads it once the
    // transaction that could not see it has ended.
    let mut fresh = session(&address, port).await;
    for (whose, reading) in [
        ("the session that held the snapshot", &mut reader),
        ("the session that committed", &mut writer),
        ("a session opened afterwards", &mut fresh),
    ] {
        let seen = column(case, reading, select).await;
        assert_eq!(
            seen,
            vec!["second".to_string()],
            "case {case:?}: {whose} must read the committed value, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_transaction_carries_over_the_schema_it_changed() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-isolation-ddl-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut writer = session(&address, port).await;
    let case = "transactional DDL";
    run(
        case,
        &mut writer,
        "CREATE TABLE stock (id integer PRIMARY KEY, name text)",
    )
    .await;
    run(case, &mut writer, "INSERT INTO stock VALUES (1, 'bolt')").await;

    // PostgreSQL's DDL is transactional: the column a committed transaction added is there
    // for every later session, and the one a rolled-back transaction added was never there.
    run(case, &mut writer, "BEGIN").await;
    run(
        case,
        &mut writer,
        "ALTER TABLE stock ADD COLUMN quantity integer",
    )
    .await;
    run(case, &mut writer, "UPDATE stock SET quantity = 7 WHERE id = 1").await;
    run(
        case,
        &mut writer,
        "INSERT INTO stock VALUES (2, 'nut', 11)",
    )
    .await;
    run(case, &mut writer, "COMMIT").await;
    let mut fresh = session(&address, port).await;
    let seen = column(
        case,
        &mut fresh,
        "SELECT quantity FROM stock ORDER BY id",
    )
    .await;
    assert_eq!(
        seen,
        vec!["7".to_string(), "11".to_string()],
        "case {case:?}: a session opened after the commit must read the column the \
         transaction added and the rows it wrote, got {seen:?}"
    );

    // A table and the objects that hang off it, created in ONE transaction: the table is
    // declared before the index and the view that name it, whatever their names are.
    let case = "a table and its dependent objects in one transaction";
    run(case, &mut writer, "BEGIN").await;
    run(
        case,
        &mut writer,
        "CREATE TABLE users (id integer PRIMARY KEY, email text)",
    )
    .await;
    run(
        case,
        &mut writer,
        "CREATE INDEX idx_users_email ON users (email)",
    )
    .await;
    run(
        case,
        &mut writer,
        "CREATE VIEW alphabetical AS SELECT email FROM users ORDER BY email",
    )
    .await;
    run(
        case,
        &mut writer,
        "INSERT INTO users VALUES (1, 'b@example.com'), (2, 'a@example.com')",
    )
    .await;
    run(case, &mut writer, "COMMIT").await;
    let mut after = session(&address, port).await;
    let seen = column(case, &mut after, "SELECT email FROM alphabetical").await;
    assert_eq!(
        seen,
        vec!["a@example.com".to_string(), "b@example.com".to_string()],
        "case {case:?}: a session opened after the commit must read the table, the view and \
         the rows the transaction created, got {seen:?}"
    );
    let indexed = column(
        case,
        &mut after,
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_users_email'",
    )
    .await;
    assert_eq!(
        indexed,
        vec!["idx_users_email".to_string()],
        "case {case:?}: the index the transaction created must be carried over with its \
         table, got {indexed:?}"
    );

    let case = "transactional DDL";
    run(case, &mut writer, "BEGIN").await;
    run(case, &mut writer, "ALTER TABLE stock ADD COLUMN bin text").await;
    run(case, &mut writer, "ROLLBACK").await;
    let refused = fresh
        .query("SELECT bin FROM stock")
        .await
        .expect_err("the column a rolled-back transaction added must not be there");
    assert!(
        refused.message.contains("bin"),
        "case {case:?}: reading a column no transaction committed must be refused naming it, \
         got {refused:?}"
    );

    // A commit carries over the schema the COMMITTING transaction changed and nothing else:
    // what another session created and committed while it was open is still there, with its
    // rows.
    let case = "another session's committed DDL";
    run(case, &mut writer, "BEGIN").await;
    run(case, &mut writer, "INSERT INTO stock VALUES (3, 'washer', 5)").await;
    run(
        case,
        &mut after,
        "CREATE TABLE suppliers (id integer PRIMARY KEY, name text)",
    )
    .await;
    run(
        case,
        &mut after,
        "INSERT INTO suppliers VALUES (1, 'northern')",
    )
    .await;
    run(case, &mut writer, "COMMIT").await;
    let mut last = session(&address, port).await;
    let seen = column(case, &mut last, "SELECT name FROM suppliers ORDER BY id").await;
    assert_eq!(
        seen,
        vec!["northern".to_string()],
        "case {case:?}: a table another session created and committed must survive the \
         commit of a transaction that never touched it, got {seen:?}"
    );
    let stocked = column(case, &mut last, "SELECT name FROM stock ORDER BY id").await;
    assert_eq!(
        stocked,
        vec![
            "bolt".to_string(),
            "nut".to_string(),
            "washer".to_string()
        ],
        "case {case:?}: the committing transaction's own row must be there too, got \
         {stocked:?}"
    );
}

#[tokio::test]
async fn two_transactions_writing_different_rows_both_commit_their_own_value() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-isolation-skew-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut first = session(&address, port).await;
    let mut second = session(&address, port).await;
    let case = "a concurrent pair over different rows";
    run(
        case,
        &mut first,
        "CREATE TABLE pieces (id integer PRIMARY KEY, colour text)",
    )
    .await;
    run(
        case,
        &mut first,
        "INSERT INTO pieces VALUES (1, 'black'), (2, 'white')",
    )
    .await;

    // The classic pair: each transaction reads the other row and writes its own with what
    // it read. Each one reads the rows as they stood when it began, so the two colours end
    // up swapped — neither transaction saw the other's uncommitted write.
    run(case, &mut first, "BEGIN").await;
    run(case, &mut second, "BEGIN").await;
    run(
        case,
        &mut first,
        "UPDATE pieces SET colour = (SELECT colour FROM pieces WHERE id = 2) WHERE id = 1",
    )
    .await;
    run(
        case,
        &mut second,
        "UPDATE pieces SET colour = (SELECT colour FROM pieces WHERE id = 1) WHERE id = 2",
    )
    .await;
    run(case, &mut first, "COMMIT").await;
    run(case, &mut second, "COMMIT").await;

    let mut fresh = session(&address, port).await;
    let seen = column(case, &mut fresh, "SELECT colour FROM pieces ORDER BY id").await;
    assert_eq!(
        seen,
        vec!["white".to_string(), "black".to_string()],
        "case {case:?}: each row must hold the value its own transaction wrote, got {seen:?}"
    );
}

#[tokio::test]
async fn the_serializable_level_refuses_the_pair_the_lower_levels_permit() {
    let router = emulator();
    let (address, port) = available_instance(
        &router,
        "emu-isolation-serializable-postgres",
        "postgres",
        &params!["EngineVersion" => "16.4"],
    )
    .await;
    let mut setup = session(&address, port).await;
    run(
        "setup",
        &mut setup,
        "CREATE TABLE tallies (id integer PRIMARY KEY, kind text, amount integer)",
    )
    .await;

    // The rows span the levels a client can ask for: the pair below writes different rows
    // out of what it read of the other's, which SERIALIZABLE cannot order one before the
    // other — so one of the two is refused — and which every lower level permits, each
    // transaction's own write standing.
    for (case, level, refusals, expected) in [
        ("serializable", "SERIALIZABLE", 1usize, None),
        (
            "repeatable read",
            "REPEATABLE READ",
            0,
            Some(vec!["2", "1"]),
        ),
        ("read committed", "READ COMMITTED", 0, Some(vec!["2", "1"])),
    ] {
        run(case, &mut setup, "DELETE FROM tallies").await;
        run(
            case,
            &mut setup,
            "INSERT INTO tallies VALUES (1, 'first', 1), (2, 'second', 2)",
        )
        .await;
        let mut first = session(&address, port).await;
        let mut second = session(&address, port).await;
        for opening in [&mut first, &mut second] {
            run(
                case,
                opening,
                &format!("BEGIN TRANSACTION ISOLATION LEVEL {level}"),
            )
            .await;
        }
        // Each reads the other's row and writes its own with what it read.
        run(
            case,
            &mut first,
            "UPDATE tallies SET amount = (SELECT amount FROM tallies WHERE id = 2) \
             WHERE id = 1",
        )
        .await;
        run(
            case,
            &mut second,
            "UPDATE tallies SET amount = (SELECT amount FROM tallies WHERE id = 1) \
             WHERE id = 2",
        )
        .await;
        let mut refused: Vec<String> = Vec::new();
        for committing in [&mut first, &mut second] {
            if let Err(refusal) = committing.query("COMMIT").await {
                refused.push(refusal.code.clone());
            }
        }
        assert_eq!(
            refused.len(),
            refusals,
            "case {case:?}: expected {refusals} of the two transactions to be refused, got \
             {refused:?}"
        );
        for code in &refused {
            assert_eq!(
                code, "40001",
                "case {case:?}: a transaction the server could not serialize must be refused \
                 under SQLSTATE 40001, got {code:?}"
            );
        }
        if let Some(expected) = expected {
            let mut reader = session(&address, port).await;
            let seen = column(case, &mut reader, "SELECT amount FROM tallies ORDER BY id").await;
            assert_eq!(
                seen, expected,
                "case {case:?}: each row must hold the value its own transaction wrote, got \
                 {seen:?}"
            );
        }
    }
}
