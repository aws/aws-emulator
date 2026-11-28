// SPDX-License-Identifier: CC0-1.0

//! The SQL SEMANTICS a session on a cluster's endpoint is served under: what a
//! client of Aurora DSQL sees that a client of a stock PostgreSQL server does
//! not.
//!
//! The RULE under test: the endpoint a cluster publishes serves the SEMANTICS of
//! the product behind it, not the engine's own.
//!
//! * a session reports the isolation a transaction runs at unless it asks for
//!   another — `repeatable read`, which a client reads with
//!   `current_setting('default_transaction_isolation')` and `SHOW` — and
//!   `version()` names the PostgreSQL version the engine is compatible with;
//! * two sessions that write the SAME row do not hold each other up: the second
//!   `UPDATE` answers rather than waiting for the first transaction to end,
//!   exactly one of the two commits stands, and the other is refused with the
//!   `SQLSTATE` of a serialization failure at severity `ERROR`, leaving no
//!   effect at all — after which the same write, run serially, commits;
//! * a transaction reads the database as it stood when it BEGAN, however often
//!   anybody else commits meanwhile;
//! * a feature this engine does not implement is refused under the class its own
//!   condition has, and leaves no relation behind in the catalog, while an
//!   ordinary `CREATE TABLE` beside it succeeds.
//!
//! Everything runs in ONE process, over the wire itself: the client, the cluster
//! and the endpoint it publishes are the ones
//! [`crate::cluster_data_plane_session`] opens, and the statements below are the
//! statements a stock libpq client would send.

use std::time::{Duration, SystemTime};

use crate::cluster_data_plane_session::{
    auth_token, login, Cluster, PostgresSession, Refusal, Transport, ADMIN_USER, CLUSTER_DATABASE,
};

/// The isolation level every transaction on a cluster runs at, which is what
/// `default_transaction_isolation` reports.
const REPEATABLE_READ: &str = "repeatable read";

/// The `SQLSTATE` and the sentence the losing commit of a write conflict carries.
const SERIALIZATION_FAILURE: &str = "40001";
const WRITE_CONFLICT: &str = "change conflicts with another transaction (OC000)";

/// The `SQLSTATE` a type this engine does not have, and a construct it does not
/// implement, are refused under.
const UNDEFINED_OBJECT: &str = "42704";
const FEATURE_NOT_SUPPORTED: &str = "0A000";

/// How long a statement that must NOT wait for another transaction is given
/// before the case gives up on it.
///
/// Generous on purpose: what the case distinguishes is a statement that ANSWERS
/// from one that waits for a transaction nobody is ending, which no run of a
/// loaded host turns into the other. It is not a measurement of how fast the
/// statement is.
const ANSWERS_WITHIN: Duration = Duration::from_secs(60);

/// A session on `cluster`'s endpoint, as the admin role with a fresh token.
async fn admin_session(case: &str, cluster: &Cluster, endpoint: &str) -> PostgresSession {
    let token = auth_token(
        "DbConnectAdmin",
        endpoint,
        SystemTime::now(),
        Duration::from_secs(900),
    );
    login(
        case,
        cluster.served_port(),
        Transport::Tls,
        ADMIN_USER,
        &token,
        Some(CLUSTER_DATABASE),
    )
    .await
    .unwrap_or_else(|refusal| panic!("case {case}: a session must open, got {refusal:?}"))
}

/// Run `statement` and answer the one value it reported, panicking with the
/// server's own refusal where it refused.
async fn value_of(case: &str, session: &mut PostgresSession, statement: &str) -> Option<String> {
    let answered = session
        .query(statement)
        .await
        .unwrap_or_else(|refusal| panic!("case {case}: {statement} must run, got {refusal:?}"));
    answered.only().map(str::to_string)
}

/// Run `statement`, expecting the server to refuse it, and answer the refusal.
async fn refusal_of(case: &str, session: &mut PostgresSession, statement: &str) -> Refusal {
    match session.query(statement).await {
        Ok(answered) => {
            panic!("case {case}: {statement} must be refused, but answered {answered:?}")
        }
        Err(refusal) => refusal,
    }
}

/// RULE: a session reports the semantics of the PRODUCT behind the endpoint —
/// the isolation a transaction runs at unless it asks for another, and a
/// `version()` naming the PostgreSQL version the engine is compatible with.
///
/// The rows span both ways a client reads a configuration parameter
/// (`current_setting()` and `SHOW`) and both parameters that carry the level.
#[tokio::test]
async fn a_session_reports_this_engines_isolation_and_version() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let mut session = admin_session("the isolation a cluster reports", &cluster, &endpoint).await;

    // (case, the statement, the value it must report)
    let rows: &[(&str, &str, &str)] = &[
        (
            "the default level, read as a function",
            "SELECT current_setting('default_transaction_isolation')",
            REPEATABLE_READ,
        ),
        (
            "the default level, read with SHOW",
            "SHOW default_transaction_isolation",
            REPEATABLE_READ,
        ),
        (
            "the level this transaction runs at, read as a function",
            "SELECT current_setting('transaction_isolation')",
            REPEATABLE_READ,
        ),
        (
            "the level this transaction runs at, read with SHOW",
            "SHOW transaction_isolation",
            REPEATABLE_READ,
        ),
        (
            "the default level, asked for with the missing-ok form",
            "SELECT current_setting('default_transaction_isolation', true)",
            REPEATABLE_READ,
        ),
        (
            "a parameter this session holds no value for, asked that way",
            "SELECT coalesce(current_setting('not_a_parameter', true), 'no value')",
            "no value",
        ),
    ];
    for (case, statement, want) in rows {
        let reported = value_of(case, &mut session, statement).await;
        assert_eq!(
            reported.as_deref(),
            Some(*want),
            "case {case:?}: expected {want:?} from {statement:?}, got {reported:?}"
        );
    }

    // The server identifies itself as the engine it is compatible with, which is
    // what a driver and an application read to decide how to talk to it.
    let version = value_of("the version sentence", &mut session, "SELECT version()")
        .await
        .unwrap_or_default();
    assert!(
        version.starts_with("PostgreSQL "),
        "case the version sentence: it must name the PostgreSQL version first, got {version:?}"
    );
    let reported = session
        .parameter("server_version")
        .expect("the backend reports server_version");
    assert!(
        version.contains(&reported),
        "case the version sentence: it must carry the version the backend reports \
         ({reported:?}), got {version:?}"
    );
}

/// RULE: two sessions writing the SAME row do not hold each other up, exactly
/// one of the two commits stands, and the other is refused a serialization
/// failure and leaves no effect — after which the same write, run serially,
/// commits.
#[tokio::test]
async fn two_sessions_writing_one_row_leave_exactly_one_commit_standing() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let mut owner = admin_session("the row's first writer", &cluster, &endpoint).await;
    for statement in [
        "CREATE TABLE ledger (id INTEGER PRIMARY KEY, who TEXT)",
        "INSERT INTO ledger (id, who) VALUES (1, 'before')",
    ] {
        value_of("the seed", &mut owner, statement).await;
    }

    let mut first = admin_session("the first transaction", &cluster, &endpoint).await;
    let mut second = admin_session("the second transaction", &cluster, &endpoint).await;
    let mut reader = admin_session("an independent reader", &cluster, &endpoint).await;
    for (case, session) in [
        ("the first transaction", &mut first),
        ("the second transaction", &mut second),
    ] {
        value_of(case, session, "BEGIN").await;
    }

    // Neither UPDATE waits for the other: an engine that held the row would
    // leave the second one waiting for a transaction nobody is ending.
    for (case, session, who) in [
        ("the first writer", &mut first, "first"),
        ("the second writer", &mut second, "second"),
    ] {
        let statement = format!("UPDATE ledger SET who = '{who}' WHERE id = 1");
        let answered = ::tokio::time::timeout(ANSWERS_WITHIN, session.query(&statement))
            .await
            .unwrap_or_else(|_| {
                panic!("case {case}: {statement} must answer rather than wait for the other \
                        transaction")
            })
            .unwrap_or_else(|refusal| panic!("case {case}: {statement} must run, got {refusal:?}"));
        assert_eq!(
            answered.tag, "UPDATE 1",
            "case {case}: the UPDATE must report the one row it changed, got {answered:?}"
        );
    }

    // Until one of them commits, a session outside both reads the row as it was.
    let read = value_of(
        "the uncommitted window",
        &mut reader,
        "SELECT who FROM ledger WHERE id = 1",
    )
    .await;
    assert_eq!(
        read.as_deref(),
        Some("before"),
        "case the uncommitted window: a reader must see the row as it was, got {read:?}"
    );

    value_of("the winning commit", &mut first, "COMMIT").await;
    let refused = refusal_of("the losing commit", &mut second, "COMMIT").await;
    assert_eq!(
        (
            refused.code.as_str(),
            refused.severity.as_str(),
            refused.message.as_str()
        ),
        (SERIALIZATION_FAILURE, "ERROR", WRITE_CONFLICT),
        "case the losing commit: expected {SERIALIZATION_FAILURE} / ERROR / {WRITE_CONFLICT:?}, \
         got {refused:?}"
    );

    // The loser left nothing behind: the row carries the winner's value, and the
    // table holds the one row it held.
    for (case, statement, want) in [
        (
            "the surviving value",
            "SELECT who FROM ledger WHERE id = 1",
            "first",
        ),
        ("the rows the table holds", "SELECT count(*) FROM ledger", "1"),
    ] {
        let read = value_of(case, &mut reader, statement).await;
        assert_eq!(
            read.as_deref(),
            Some(want),
            "case {case:?}: expected {want:?}, got {read:?}"
        );
    }

    // And the refused write, run serially, commits.
    for statement in [
        "BEGIN",
        "UPDATE ledger SET who = 'second' WHERE id = 1",
        "COMMIT",
    ] {
        value_of("the serial retry", &mut second, statement).await;
    }
    let read = value_of(
        "after the serial retry",
        &mut reader,
        "SELECT who FROM ledger WHERE id = 1",
    )
    .await;
    assert_eq!(
        read.as_deref(),
        Some("second"),
        "case after the serial retry: the retried write must stand, got {read:?}"
    );
}

/// RULE: a transaction reads the database as it stood when it BEGAN — a row
/// another session commits after that is invisible to it until it ends — two
/// transactions writing DIFFERENT rows both commit, and what a commit carries is
/// the value the transaction computed on its own snapshot.
#[tokio::test]
async fn a_transaction_reads_the_database_as_it_stood_when_it_began() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let mut owner = admin_session("the seed", &cluster, &endpoint).await;
    for statement in [
        "CREATE TABLE notes (id INTEGER PRIMARY KEY, note TEXT)",
        "INSERT INTO notes (id, note) VALUES (1, 'first')",
    ] {
        value_of("the seed", &mut owner, statement).await;
    }

    let mut held = admin_session("a transaction open across a commit", &cluster, &endpoint).await;
    let mut other = admin_session("another writer", &cluster, &endpoint).await;
    value_of("a transaction open across a commit", &mut held, "BEGIN").await;
    for statement in [
        "BEGIN",
        "INSERT INTO notes (id, note) VALUES (2, 'second')",
        "COMMIT",
    ] {
        value_of("another writer", &mut other, statement).await;
    }
    let read = value_of(
        "the snapshot",
        &mut held,
        "SELECT count(*) FROM notes",
    )
    .await;
    assert_eq!(
        read.as_deref(),
        Some("1"),
        "case the snapshot: a transaction must not see a row committed after it began, got \
         {read:?}"
    );

    // Its own write of ANOTHER row commits beside the one it could not see, and
    // what it commits is the value it computed on its own snapshot — not one
    // recomputed from the rows the other session committed meanwhile.
    for statement in [
        "INSERT INTO notes (id, note) VALUES (3, 'third')",
        "INSERT INTO notes (id, note) SELECT 4, count(*) FROM notes",
        "COMMIT",
    ] {
        value_of("a write of another row", &mut held, statement).await;
    }
    let read = value_of(
        "the committed value",
        &mut held,
        "SELECT note FROM notes WHERE id = 4",
    )
    .await;
    assert_eq!(
        read.as_deref(),
        Some("2"),
        "case the committed value: the row must carry what the transaction computed on its own \
         snapshot (the two rows it could see), got {read:?}"
    );
    let read = value_of(
        "after both commits",
        &mut held,
        "SELECT count(*) FROM notes",
    )
    .await;
    assert_eq!(
        read.as_deref(),
        Some("4"),
        "case after both commits: writes of different rows must both stand, got {read:?}"
    );
}

/// RULE: a feature this engine does not implement is refused under the class its
/// own condition has, and leaves no relation in the catalog — while the ordinary
/// form of the same statement succeeds and is reported there.
///
/// The rows span the whole `serial` family and both spellings of each (a type
/// that denotes a sequence, which this engine has none of), the temporary
/// relations, and a sequence of its own.
#[tokio::test]
async fn a_feature_this_engine_does_not_implement_is_refused_and_creates_nothing() {
    let cluster = Cluster::created().await;
    let endpoint = cluster.waited_for("ACTIVE").await;
    let mut session = admin_session("the refusals", &cluster, &endpoint).await;

    // (case, the statement, the relation it would have created, the SQLSTATE)
    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "a serial key",
            "CREATE TABLE serial_key (id SERIAL PRIMARY KEY, note TEXT)",
            "serial_key",
            UNDEFINED_OBJECT,
        ),
        (
            "a bigserial column",
            "CREATE TABLE big_key (id BIGSERIAL, note TEXT)",
            "big_key",
            UNDEFINED_OBJECT,
        ),
        (
            "a smallserial column",
            "CREATE TABLE small_key (id SMALLSERIAL)",
            "small_key",
            UNDEFINED_OBJECT,
        ),
        (
            "the serial4 spelling",
            "CREATE TABLE spelled_four (id SERIAL4)",
            "spelled_four",
            UNDEFINED_OBJECT,
        ),
        (
            "the serial8 spelling",
            "CREATE TABLE spelled_eight (id SERIAL8)",
            "spelled_eight",
            UNDEFINED_OBJECT,
        ),
        (
            "a temporary table",
            "CREATE TEMP TABLE scratch (id INTEGER)",
            "scratch",
            FEATURE_NOT_SUPPORTED,
        ),
        (
            "a temporary table, spelled in full",
            "CREATE TEMPORARY TABLE scratch_too (id INTEGER)",
            "scratch_too",
            FEATURE_NOT_SUPPORTED,
        ),
        (
            "a sequence of its own",
            "CREATE SEQUENCE counted",
            "counted",
            FEATURE_NOT_SUPPORTED,
        ),
    ];
    for (case, statement, relation, want) in rows {
        let refused = refusal_of(case, &mut session, statement).await;
        assert_eq!(
            (refused.code.as_str(), refused.severity.as_str()),
            (*want, "ERROR"),
            "case {case:?}: expected {want} / ERROR for {statement:?}, got {refused:?}"
        );
        assert!(
            !refused.message.is_empty(),
            "case {case:?}: the refusal must say what was refused, got {refused:?}"
        );
        // Nothing was created: the catalog the client reads holds no such
        // relation.
        let held = value_of(
            case,
            &mut session,
            &format!(
                "SELECT count(*) FROM information_schema.tables WHERE table_name = '{relation}'"
            ),
        )
        .await;
        assert_eq!(
            held.as_deref(),
            Some("0"),
            "case {case:?}: the refused statement must leave no relation {relation:?} behind, \
             got {held:?}"
        );
    }

    // The ordinary form of the same statement succeeds, and the catalog reports
    // it: the refusals above are about the construct and not about the endpoint.
    value_of(
        "an ordinary table",
        &mut session,
        "CREATE TABLE ordinary (id INTEGER PRIMARY KEY, note TEXT)",
    )
    .await;
    let held = value_of(
        "an ordinary table",
        &mut session,
        "SELECT count(*) FROM information_schema.tables WHERE table_name = 'ordinary'",
    )
    .await;
    assert_eq!(
        held.as_deref(),
        Some("1"),
        "case an ordinary table: the catalog must report the relation that was created, got \
         {held:?}"
    );
}
