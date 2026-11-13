// SPDX-License-Identifier: CC0-1.0

//! What belongs to ONE connection of the Data API, driven over the wire through
//! the shared harness in [`super::emulator`]: every call below is a real `POST
//! /` with `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the
//! emulator's HTTP behaviour and not a handler's return value.
//!
//! A submission that names a `SessionId` runs on the connection that session
//! is, and one that names none runs on a connection opened for it and closed
//! when it ends. Two rules follow, and both are asserted over a TABLE rather
//! than over one case.
//!
//! 1. A TEMPORARY relation belongs to the connection that created it: a later
//!    statement of the same session resolves it, and a statement on any other
//!    connection cannot resolve it at all and FAILS. The rows differ in the
//!    spelling that creates the relation, in the kind of relation, and in the
//!    form the later statement names it by. Its complement is asserted too: a
//!    PERMANENT table one connection creates is the database's, so every
//!    connection reads it.
//!
//! 2. A lock a connection takes inside an open transaction is held until that
//!    transaction ends, and a statement on another connection that needs a
//!    CONFLICTING lock on the same table cannot run while it is held: it stays
//!    non-terminal, so a caller can see it running and cancel it, and it runs
//!    as soon as the holder commits. The rows differ in the mode the holder
//!    holds and in what the other connection submits, which together decide
//!    whether the two conflict at all.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::emulator::{
    error_code, id_of, list, wait_for_terminal, Emulator, TERMINAL_STATUSES, WAIT_SECONDS,
};

/// How long a session below is kept alive: longer than any case takes, so a
/// session is closed because a case closed it and never because the clock ran
/// out while a loaded machine was running the test.
const KEEP_ALIVE_SECONDS: i32 = 600;

/// How long a poll below waits for a statement to become observable as running.
/// Generous, and well inside the wait the emulator gives a statement blocked on
/// a lock, so a statement that is waiting is still waiting when it is observed.
const RUNNING_BUDGET: Duration = Duration::from_secs(15);

/// How long to wait between two polls of a statement's status.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The connection a submission that names no session is made with: one
/// documented authorization combination (a provisioned cluster as a database
/// user).
fn connection() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// Open a session by asking to keep one alive, running `sql` in it, and answer
/// its `SessionId`.
async fn open_session(emulator: &Emulator, case: &str, sql: &str) -> String {
    let mut request = connection();
    request["Sql"] = json!(sql);
    request["SessionKeepAliveSeconds"] = json!(KEEP_ALIVE_SECONDS);
    request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
    let opened = emulator.ok("ExecuteStatement", &request).await;
    let described = wait_for_terminal(emulator, case, &id_of(&opened)).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "case {case}: the statement that opens a session must run, got {described}"
    );
    opened["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a kept-alive submission reports its SessionId: \
                                  {opened}"))
        .to_string()
}

/// Run `sql` in the session `session`, wait for it, and answer the record it
/// reached.
async fn run_in_session(emulator: &Emulator, case: &str, session: &str, sql: &str) -> Value {
    let submitted = emulator
        .execute_and_wait(&json!({"SessionId": session, "Sql": sql}))
        .await;
    wait_for_terminal(emulator, case, &id_of(&submitted)).await
}

/// Run `sql` on a connection of its own — no `SessionId` — wait for it, and
/// answer the record it reached.
async fn run_unsessioned(emulator: &Emulator, case: &str, sql: &str) -> Value {
    let mut request = connection();
    request["Sql"] = json!(sql);
    let submitted = emulator.execute_and_wait(&request).await;
    wait_for_terminal(emulator, case, &id_of(&submitted)).await
}

/// The single `longValue` the statement's result carries.
async fn single_value(emulator: &Emulator, case: &str, described: &Value) -> i64 {
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": described["Id"]}))
        .await;
    let records = list(&fetched, "Records");
    assert_eq!(
        records.len(),
        1,
        "case {case}: expected one row, got {fetched}"
    );
    records[0]
        .as_array()
        .and_then(|cells| cells.first())
        .and_then(|cell| cell["longValue"].as_i64())
        .unwrap_or_else(|| panic!("case {case}: expected one numeric field, got {fetched}"))
}

/// One row of the temporary-relation table: what the owning session builds, and
/// how a later statement names it.
struct TempCase {
    label: &'static str,
    /// The statement that creates the temporary relation, with `{}` standing
    /// for the relation's name.
    create: &'static str,
    /// The statements that fill it, each with `{}` standing for that name.
    fill: &'static [&'static str],
    /// How a later statement reads it, with `{}` standing for that name.
    read: &'static str,
    /// The value that read answers with.
    expected: i64,
}

/// A temporary relation belongs to the CONNECTION that created it: the session
/// that built it reads it, and a statement on any other connection fails
/// because the relation is not there for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_temporary_relation_belongs_to_the_connection_that_created_it() {
    let cases = [
        TempCase {
            label: "a TEMP table, read by its bare name",
            create: "CREATE TEMP TABLE {} (id INTEGER)",
            fill: &["INSERT INTO {} VALUES (42)"],
            read: "SELECT id FROM {}",
            expected: 42,
        },
        TempCase {
            label: "a TEMPORARY table, read by its bare name",
            create: "CREATE TEMPORARY TABLE {} (id INTEGER)",
            fill: &["INSERT INTO {} VALUES (7)"],
            read: "SELECT id FROM {}",
            expected: 7,
        },
        TempCase {
            label: "a TEMP table, read through the session's own temporary schema",
            create: "CREATE TEMP TABLE {} (id INTEGER)",
            fill: &["INSERT INTO {} VALUES (11)"],
            read: "SELECT id FROM pg_temp.{}",
            expected: 11,
        },
        TempCase {
            label: "a TEMP table, read through its own qualified column",
            create: "CREATE TEMP TABLE {} (id INTEGER)",
            fill: &["INSERT INTO {} VALUES (13)"],
            read: "SELECT {}.id FROM {}",
            expected: 13,
        },
        TempCase {
            label: "a TEMP table built by one statement of a multi-statement text",
            create: "CREATE TEMP TABLE {} (id INTEGER); INSERT INTO {} VALUES (23)",
            fill: &[],
            read: "SELECT max(id) FROM {}",
            expected: 23,
        },
        TempCase {
            label: "a TEMP table aggregated over",
            create: "CREATE TEMP TABLE {} (id INTEGER)",
            fill: &["INSERT INTO {} VALUES (1), (2), (3)"],
            read: "SELECT count(*) FROM {}",
            expected: 3,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let emulator = Emulator::new();
        let relation = format!("sess_tmp_{index}");
        let fill = |sql: &str| sql.replace("{}", &relation);

        // The session that owns the relation builds it.
        let session = open_session(&emulator, case.label, &fill(case.create)).await;
        for statement in case.fill {
            let described = run_in_session(&emulator, case.label, &session, &fill(statement)).await;
            assert_eq!(
                described["Status"], "FINISHED",
                "case {}: filling the session's own relation must FINISH, got {described}",
                case.label
            );
        }

        // Its own session reads it.
        let read = fill(case.read);
        let owner = run_in_session(&emulator, case.label, &session, &read).await;
        assert_eq!(
            owner["Status"], "FINISHED",
            "case {}: the session that built the relation must read it, got {owner}",
            case.label
        );
        let value = single_value(&emulator, case.label, &owner).await;
        assert_eq!(
            value, case.expected,
            "case {}: the owning session must read {}, got {value}",
            case.label, case.expected
        );

        // A statement on a connection of its own cannot: the relation is not
        // in its temporary schema, so the statement fails naming it.
        let fresh = run_unsessioned(&emulator, case.label, &read).await;
        assert_eq!(
            fresh["Status"], "FAILED",
            "case {}: a statement on another connection must not resolve another \
             connection's temporary relation, got {fresh}",
            case.label
        );
        let error = fresh["Error"].as_str().unwrap_or_default().to_ascii_lowercase();
        assert!(
            error.contains(&relation),
            "case {}: the failure must name the relation {relation:?} it could not resolve, got \
             {fresh}",
            case.label
        );
        assert_eq!(
            fresh["HasResultSet"],
            json!(false),
            "case {}: a statement that failed has no result set, got {fresh}",
            case.label
        );

        // And neither can another session: a session is a connection too.
        let other = open_session(&emulator, case.label, "SELECT 1").await;
        assert_ne!(
            other, session,
            "case {}: the second session must be a session of its own",
            case.label
        );
        let elsewhere = run_in_session(&emulator, case.label, &other, &read).await;
        assert_eq!(
            elsewhere["Status"], "FAILED",
            "case {}: another session must not resolve this session's temporary relation, got \
             {elsewhere}",
            case.label
        );
    }
}

/// The complement of the rule: a PERMANENT table belongs to the DATABASE, so a
/// statement on any connection reads what another connection created — the
/// isolation above is the temporary schema's and not the connection's whole
/// view of the database.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_permanent_table_one_connection_creates_is_read_by_every_connection() {
    let emulator = Emulator::new();
    let session = open_session(
        &emulator,
        "a permanent table",
        "CREATE TABLE shared_rows (id INTEGER)",
    )
    .await;
    let filled = run_in_session(
        &emulator,
        "a permanent table",
        &session,
        "INSERT INTO shared_rows VALUES (99)",
    )
    .await;
    assert_eq!(
        filled["Status"], "FINISHED",
        "filling a permanent table must FINISH, got {filled}"
    );

    let fresh = run_unsessioned(
        &emulator,
        "a permanent table",
        "SELECT id FROM shared_rows",
    )
    .await;
    assert_eq!(
        fresh["Status"], "FINISHED",
        "a permanent table is the database's, so another connection reads it, got {fresh}"
    );
    let value = single_value(&emulator, "a permanent table", &fresh).await;
    assert_eq!(
        value, 99,
        "another connection must read the row the session inserted, got {value}"
    );
}

/// One row of the lock table: the mode the holder's session takes on the table,
/// what the other connection then submits, and whether the two conflict.
///
/// Which pairs conflict is the conflicting-lock-modes table of the PostgreSQL
/// *Explicit Locking* documentation, which Amazon Redshift's `LOCK` is derived
/// from: a reading statement takes `ACCESS SHARE`, which only `ACCESS
/// EXCLUSIVE` conflicts with, and an `INSERT` takes `ROW EXCLUSIVE`, which
/// `SHARE`, `SHARE ROW EXCLUSIVE`, `EXCLUSIVE` and `ACCESS EXCLUSIVE` conflict
/// with.
struct LockCase {
    /// The mode the holder names, or `None` to name none at all — which is the
    /// `ACCESS EXCLUSIVE` Amazon Redshift documents `LOCK` as taking.
    held: Option<&'static str>,
    /// What the other connection submits against the same table.
    submitted: &'static str,
    /// Whether that submission has to wait for the lock.
    blocked: bool,
}

/// The statements the other connection submits, and the mode each takes.
const READ: &str = "SELECT id FROM contended_lock";
const WRITE: &str = "INSERT INTO contended_lock VALUES (2)";

/// A statement that needs a lock another connection holds inside an open
/// transaction stays non-terminal until it is cancelled, and one that needs no
/// conflicting lock runs straight through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_conflicting_lock_holds_a_statement_non_terminal_and_cancellable() {
    let cases = [
        LockCase {
            held: Some("ACCESS SHARE"),
            submitted: READ,
            blocked: false,
        },
        LockCase {
            held: Some("ROW SHARE"),
            submitted: READ,
            blocked: false,
        },
        LockCase {
            held: Some("ROW EXCLUSIVE"),
            submitted: READ,
            blocked: false,
        },
        LockCase {
            held: Some("SHARE UPDATE EXCLUSIVE"),
            submitted: READ,
            blocked: false,
        },
        LockCase {
            held: Some("SHARE"),
            submitted: READ,
            blocked: false,
        },
        LockCase {
            held: Some("EXCLUSIVE"),
            submitted: READ,
            blocked: false,
        },
        LockCase {
            held: Some("ACCESS EXCLUSIVE"),
            submitted: READ,
            blocked: true,
        },
        LockCase {
            held: None,
            submitted: READ,
            blocked: true,
        },
        LockCase {
            held: Some("ACCESS SHARE"),
            submitted: WRITE,
            blocked: false,
        },
        LockCase {
            held: Some("ROW EXCLUSIVE"),
            submitted: WRITE,
            blocked: false,
        },
        LockCase {
            held: Some("SHARE"),
            submitted: WRITE,
            blocked: true,
        },
        LockCase {
            held: Some("SHARE ROW EXCLUSIVE"),
            submitted: WRITE,
            blocked: true,
        },
        LockCase {
            held: Some("EXCLUSIVE"),
            submitted: WRITE,
            blocked: true,
        },
        LockCase {
            held: Some("ACCESS EXCLUSIVE"),
            submitted: WRITE,
            blocked: true,
        },
    ];

    for case in &cases {
        let label = format!(
            "{} held while {:?} is submitted",
            case.held.unwrap_or("no mode named"),
            case.submitted
        );
        // One emulator per row, so a row never meets a lock another row left
        // behind.
        let emulator = Emulator::new();
        emulator
            .run("CREATE TABLE contended_lock (id INTEGER)")
            .await;

        // The holder's session takes the lock inside a transaction it leaves
        // open, which is what holds the lock past the statement.
        let mode = case
            .held
            .map(|mode| format!(" IN {mode} MODE"))
            .unwrap_or_default();
        let _holder = open_session(
            &emulator,
            &label,
            &format!("BEGIN; LOCK TABLE contended_lock{mode};"),
        )
        .await;

        // The other connection submits without waiting, so this test can look
        // at the statement while it is still in flight.
        let mut request = connection();
        request["Sql"] = json!(case.submitted);
        let submitted = emulator.ok("ExecuteStatement", &request).await;
        let id = id_of(&submitted);

        if !case.blocked {
            let described = wait_for_terminal(&emulator, &label, &id).await;
            assert_eq!(
                described["Status"], "FINISHED",
                "case {label}: the modes do not conflict, so the statement must run, got \
                 {described}"
            );
            continue;
        }

        // Blocked: the statement is observable as running, and it is still
        // running after a statement submitted AFTER it on the same database has
        // run to completion — so it is held by the lock and not merely waiting
        // to be scheduled, and it holds up nothing else on that database.
        let running = wait_until_running(&emulator, &label, &id).await;
        assert!(
            !TERMINAL_STATUSES.contains(&running["Status"].as_str().unwrap_or_default()),
            "case {label}: a statement waiting for a lock must not have stopped, got {running}"
        );
        let behind = run_unsessioned(&emulator, &label, "SELECT 1").await;
        assert_eq!(
            behind["Status"], "FINISHED",
            "case {label}: a statement submitted behind the blocked one must still run, got \
             {behind}"
        );
        let still = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert!(
            !TERMINAL_STATUSES.contains(&still["Status"].as_str().unwrap_or_default()),
            "case {label}: the blocked statement must still be waiting for the lock once \
             another statement of the same database has finished, got {still}"
        );
        let cancelled = emulator.ok("CancelStatement", &json!({"Id": id})).await;
        assert_eq!(
            cancelled["Status"],
            json!(true),
            "case {label}: cancelling the blocked statement must stop it, got {cancelled}"
        );
        let stopped = wait_for_terminal(&emulator, &label, &id).await;
        assert_eq!(
            stopped["Status"], "ABORTED",
            "case {label}: a cancelled statement lands ABORTED, got {stopped}"
        );

        // And a second cancel of the now-terminal statement is refused as a
        // client-input failure, naming the state it is already in and the
        // statement it asked about.
        let (status, refused) = emulator.call("CancelStatement", &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "case {label}: cancelling a statement that already stopped is a client-input \
             failure, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {label}: expected ValidationException, got {refused}"
        );
        assert_eq!(
            refused["Message"],
            json!(format!(
                "Could not cancel a query that is already in ABORTED state with ID: {id}"
            )),
            "case {label}: expected the service's own sentence, got {refused}"
        );
    }
}

/// A statement blocked on a lock runs as soon as the connection holding it
/// commits: the blocked statement holds up nothing on that database, so the
/// holder can end its transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_blocked_statement_runs_once_the_holder_commits() {
    let case = "a committed holder";
    let emulator = Emulator::new();
    emulator
        .run("CREATE TABLE contended_lock (id INTEGER)")
        .await;
    emulator.run("INSERT INTO contended_lock VALUES (5)").await;

    let holder = open_session(
        &emulator,
        case,
        "BEGIN; LOCK TABLE contended_lock IN ACCESS EXCLUSIVE MODE;",
    )
    .await;

    let mut request = connection();
    request["Sql"] = json!("SELECT id FROM contended_lock");
    let submitted = emulator.ok("ExecuteStatement", &request).await;
    let id = id_of(&submitted);
    let running = wait_until_running(&emulator, case, &id).await;
    assert!(
        !TERMINAL_STATUSES.contains(&running["Status"].as_str().unwrap_or_default()),
        "case {case}: the blocked statement must still be running, got {running}"
    );

    // The holder commits, which releases what its transaction held.
    let committed = run_in_session(&emulator, case, &holder, "COMMIT").await;
    assert_eq!(
        committed["Status"], "FINISHED",
        "case {case}: the holder's COMMIT must run while the other statement waits, got \
         {committed}"
    );

    let described = wait_for_terminal(&emulator, case, &id).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "case {case}: the statement must run once the lock is released, got {described}"
    );
    let value = single_value(&emulator, case, &described).await;
    assert_eq!(
        value, 5,
        "case {case}: the statement must read the row the table holds, got {value}"
    );
}

/// Poll `id` until it reports a status that shows the service is running it —
/// it has been chosen to be run, or its run has started — and answer the record
/// that reported it.
///
/// Polled for rather than read once: what this waits for is the CONDITION that
/// the statement is in flight, so it holds however quickly or slowly the host
/// schedules the execution.
async fn wait_until_running(emulator: &Emulator, case: &str, id: &str) -> Value {
    let started = Instant::now();
    loop {
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        let status = described["Status"].as_str().unwrap_or_default().to_string();
        if status == "PICKED" || status == "STARTED" {
            return described;
        }
        assert_eq!(
            status, "SUBMITTED",
            "case {case}: a statement waiting for a lock must be in flight, got {described}"
        );
        assert!(
            started.elapsed() < RUNNING_BUDGET,
            "case {case}: {id} never became observable as running: {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
