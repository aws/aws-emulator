// SPDX-License-Identifier: CC0-1.0

//! What a Redshift Data statement record reports about the IDENTITY it ran as and
//! the COUNTERS of what it produced, driven over the wire: every call below is a
//! real `POST /` with `X-Amz-Target: RedshiftData.<Operation>` dispatched through
//! the router, so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! case:
//!
//! * a submission that names NEITHER `DbUser` nor `SecretArn` authenticates with
//!   the temporary credentials of the identity it was signed with and runs as the
//!   database user that identity maps to, which every record of it reports; one
//!   that names a database user runs as the user it named;
//! * `ResultSize` is the size in bytes of the result a statement returned — the
//!   bytes the warehouse sent back for its rows — and 0 for a statement that
//!   finished returning none, the -1 null marker being for a statement with no
//!   size to report at all;
//! * `RedshiftQueryId` is the identifier the WAREHOUSE generated for the query,
//!   the one its query log holds that query under, and the modelled default 0 for
//!   a statement the warehouse recorded no query of its own for.

use serde_json::{json, Value};

use service_com_amazonaws_redshiftdata_redshiftdata::state::caller_database_user;

use super::emulator::{id_of, list, number, wait_for_terminal, Emulator, WAIT_SECONDS};

/// The relation the sizing cases read, and the rows it holds: three columns of
/// three different types, so a case's size depends on the TEXT each value
/// carries and not on the storage class the engine kept it in.
const GEAR_DDL: &str =
    "CREATE TABLE gear (id INTEGER NOT NULL, label VARCHAR(16) NOT NULL, amount DECIMAL(8,2) \
     NOT NULL)";
const GEAR_ROWS: &str = "INSERT INTO gear VALUES (1, 'bolt', 2.50), (22, 'washer', 10.75)";

// ---------------------------------------------------------------------------
// The database user a statement runs as
// ---------------------------------------------------------------------------

/// One row of the authorization table: the connection members a submission
/// carries, and the `DbUser` every record of that submission must then report.
struct UserCase {
    label: &'static str,
    connection: fn() -> Value,
    /// `None` for a submission whose records report no database user at all.
    expected: fn() -> Option<String>,
}

/// Every documented authorization form, against both compute targets: the
/// database user a statement reports is the one it named, or — for a submission
/// that named none — the one the caller's IAM identity maps to.
#[tokio::test]
async fn the_reported_database_user_is_the_one_the_statement_ran_as() {
    let cases = [
        UserCase {
            label: "a cluster naming a database user",
            connection: || {
                json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
            },
            expected: || Some("awsuser".to_string()),
        },
        UserCase {
            label: "a workgroup naming a database user",
            connection: || {
                json!({"WorkgroupName": "my-workgroup", "Database": "dev", "DbUser": "analyst"})
            },
            expected: || Some("analyst".to_string()),
        },
        UserCase {
            label: "a cluster authenticating with a secret",
            connection: || {
                json!({
                    "ClusterIdentifier": "my-cluster",
                    "Database": "dev",
                    "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
                })
            },
            expected: || None,
        },
        UserCase {
            label: "a cluster authenticating as the caller's IAM identity",
            connection: || json!({"ClusterIdentifier": "my-cluster", "Database": "dev"}),
            expected: || Some(caller_database_user()),
        },
        UserCase {
            label: "a workgroup authenticating as the caller's IAM identity",
            connection: || json!({"WorkgroupName": "my-workgroup", "Database": "dev"}),
            expected: || Some(caller_database_user()),
        },
    ];

    // The user an IAM identity maps to names which KIND of identity it is and
    // then that identity's own name, so it is never an empty name and never the
    // bare prefix.
    let derived = caller_database_user();
    let (prefix, name) = derived.split_once(':').unwrap_or((derived.as_str(), ""));
    assert!(
        ["IAM", "IAMR"].contains(&prefix) && !name.is_empty(),
        "the caller's database user must name the kind of identity and its name, got {derived:?}"
    );

    let emulator = Emulator::new();
    for case in &cases {
        let expected = (case.expected)();
        let expected_member = expected
            .clone()
            .map_or(Value::Null, |user| json!(user));

        // Both submit operations report the user their statement runs as, and so
        // does the record read back for either.
        let mut execute = (case.connection)();
        execute["Sql"] = json!("SELECT 1");
        execute["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        let submitted = emulator.ok("ExecuteStatement", &execute).await;

        let mut batch = (case.connection)();
        batch["Sqls"] = json!(["SELECT 1"]);
        batch["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        let batched = emulator.ok("BatchExecuteStatement", &batch).await;

        for (operation, answered) in [
            ("ExecuteStatement", &submitted),
            ("BatchExecuteStatement", &batched),
        ] {
            assert_eq!(
                answered.get("DbUser").cloned().unwrap_or(Value::Null),
                expected_member,
                "case {}: {operation} must report DbUser {expected_member}, got {answered}",
                case.label
            );
            let described = emulator
                .ok("DescribeStatement", &json!({"Id": id_of(answered)}))
                .await;
            assert_eq!(
                described.get("DbUser").cloned().unwrap_or(Value::Null),
                expected_member,
                "case {}: the record of the {operation} submission must report DbUser \
                 {expected_member}, got {described}",
                case.label
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The size of what a statement produced
// ---------------------------------------------------------------------------

/// One row of the sizing table: the statement submitted, the status it reaches,
/// and the `ResultSize` its record must report.
struct SizeCase {
    label: &'static str,
    sql: &'static str,
    status: &'static str,
    /// Counted by hand from the rows the warehouse returned: per row its own
    /// two-byte field count, and per value four bytes of length and the value's
    /// text.
    expected: i64,
}

/// `ResultSize` is the size of the result the statement returned: 0 for a
/// statement that finished returning none, the bytes the warehouse sent back for
/// one that returned rows, and the -1 null marker only for a statement that has
/// no size to report at all.
#[tokio::test]
async fn result_size_is_the_size_of_the_result_the_statement_returned() {
    let cases = [
        SizeCase {
            label: "a DDL statement, which returns no result set",
            sql: GEAR_DDL,
            status: "FINISHED",
            expected: 0,
        },
        SizeCase {
            label: "a DML statement, which returns no result set",
            sql: GEAR_ROWS,
            status: "FINISHED",
            expected: 0,
        },
        SizeCase {
            label: "one computed column of one row",
            // One row of one value: 2 bytes of row framing, 4 bytes of length
            // and the one byte of `1`.
            sql: "SELECT 1",
            status: "FINISHED",
            expected: 7,
        },
        SizeCase {
            label: "three typed columns of two rows",
            // `1`, `bolt`, `2.50` (1 + 4 + 4) and `22`, `washer`, `10.75`
            // (2 + 6 + 5): 35 bytes of text in two rows of three values, so
            // 2 * 2 bytes of row framing and 6 * 4 bytes of value lengths.
            sql: "SELECT id, label, amount FROM gear ORDER BY id",
            status: "FINISHED",
            expected: 50,
        },
        SizeCase {
            label: "a NULL, which carries no text at all",
            // One row of two values, `x` and nothing: 2 + 4 + 1 + 4 + 0.
            sql: "SELECT 'x' AS a, NULL AS b",
            status: "FINISHED",
            expected: 11,
        },
        SizeCase {
            label: "an empty result set, which returned no row at all",
            // No row was returned, so no byte was: the column it names is
            // metadata no row message carries.
            sql: "SELECT id FROM gear WHERE id < 0",
            status: "FINISHED",
            expected: 0,
        },
        SizeCase {
            label: "a statement the engine refused, which has no size to report",
            sql: "SELECT * FROM no_such_relation",
            status: "FAILED",
            expected: -1,
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let submitted = emulator
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": case.sql,
            }))
            .await;
        let described = wait_for_terminal(&emulator, case.label, &id_of(&submitted)).await;
        assert_eq!(
            described["Status"], case.status,
            "case {}: expected status {}, got {described}",
            case.label, case.status
        );
        assert_eq!(
            number(&described, "ResultSize"),
            case.expected,
            "case {}: expected ResultSize {}, got {described}",
            case.label,
            case.expected
        );
    }
}

/// A batch's statements each report the size of what THEY returned, while the
/// batch itself reports the null marker: the rows belong to its statements.
#[tokio::test]
async fn each_statement_of_a_batch_reports_its_own_size() {
    let emulator = Emulator::new();
    let batched = emulator
        .batch_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sqls": [GEAR_DDL, GEAR_ROWS, "SELECT label FROM gear ORDER BY id"],
        }))
        .await;
    let described = wait_for_terminal(&emulator, "a batch", &id_of(&batched)).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "a waited-for batch must report its terminal status, got {described}"
    );
    assert_eq!(
        number(&described, "ResultSize"),
        -1,
        "a batch has no result of its own, so its own size is null, got {described}"
    );

    // Two rows of one value, `bolt` (4) and `washer` (6): 2 * 2 bytes of row
    // framing, 2 * 4 bytes of value lengths and the 10 bytes of text.
    let expected = [0, 0, 22];
    let subs = list(&described, "SubStatements");
    assert_eq!(
        subs.len(),
        expected.len(),
        "one SubStatements entry per submitted statement, got {described}"
    );
    for (index, sub) in subs.iter().enumerate() {
        assert_eq!(
            number(sub, "ResultSize"),
            expected[index],
            "case statement {}: expected ResultSize {}, got {sub}",
            index + 1,
            expected[index]
        );
    }
}

// ---------------------------------------------------------------------------
// The query the warehouse ran a statement as
// ---------------------------------------------------------------------------

/// A statement submitted on its own reports the identifier the WAREHOUSE gave its
/// query — the one that database's query log holds it under — and the statements
/// of a batch, which the Data API reports no warehouse query id for, report the
/// member's modelled default.
#[tokio::test]
async fn the_reported_query_id_is_the_one_the_warehouse_logged() {
    let emulator = Emulator::new();

    // One statement per marker, so each one is found in the log by its own text:
    // the ids are the database's, so two statements of one database carry two
    // different ones.
    for marker in ["first_marker", "second_marker"] {
        let submitted = emulator
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": format!("SELECT 1 AS {marker}"),
            }))
            .await;
        let described = wait_for_terminal(&emulator, marker, &id_of(&submitted)).await;
        let reported = number(&described, "RedshiftQueryId");

        let logged_statement = emulator
            .run(&format!(
                "SELECT query FROM stl_query WHERE querytxt LIKE '%{marker}%' AND querytxt NOT \
                 LIKE '%stl_query%'"
            ))
            .await;
        let logged = emulator
            .ok("GetStatementResult", &json!({"Id": logged_statement}))
            .await;
        let rows = list(&logged, "Records");
        let logged_id = rows
            .first()
            .and_then(|row| row.get(0))
            .and_then(|cell| cell["longValue"].as_i64());
        assert_eq!(
            logged_id,
            Some(reported),
            "case {marker}: the reported RedshiftQueryId {reported} must be the query id the \
             database's own log holds the query under, got {logged}"
        );
        assert!(
            reported > 0,
            "case {marker}: a query the warehouse ran carries the id it gave it, got {described}"
        );
    }

    // A statement of a BATCH reports no warehouse query id at all.
    let batched = emulator
        .batch_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sqls": ["SELECT 1", "SELECT 2"],
        }))
        .await;
    let described = wait_for_terminal(&emulator, "a batch", &id_of(&batched)).await;
    assert_eq!(
        number(&described, "RedshiftQueryId"),
        0,
        "a batch is not itself a query, so no query id names it, got {described}"
    );
    for (index, sub) in list(&described, "SubStatements").iter().enumerate() {
        assert_eq!(
            number(sub, "RedshiftQueryId"),
            0,
            "case statement {}: a statement of a batch reports the modelled default, got {sub}",
            index + 1
        );
    }
}
