// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Redshift Data API statement lifecycle, driven over the
//! wire: every call below is a real `POST /` with `X-Amz-Target:
//! RedshiftData.<Operation>` dispatched through the router, so what is asserted is
//! the emulator's HTTP behaviour and not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a statement's reported rows, counters and column metadata are what the SQL
//!   engine actually produced for the SQL that was submitted;
//! * `ColumnMetadata.typeName` is the database-specific name of the column's
//!   declared type, and for a column with no declared type the name of the
//!   narrowest type that holds every value the column produced;
//! * the statement a `ExecuteStatement` creates is visible to `DescribeStatement`,
//!   `GetStatementResult`, `ListStatements` and `CancelStatement`;
//! * `ListStatements` enumerates newest-first, honours each filter and resumes
//!   exactly where the previous page stopped;
//! * an `Id` no statement carries is refused with the missing-resource error at
//!   HTTP 400 on every operation that takes one.

use serde_json::{json, Value};

use service_com_amazonaws_redshiftdata_redshiftdata::state::{
    MAX_OPEN_DATABASES, MAX_RESULT_ROWS, MAX_RETAINED_STATEMENTS,
};

// The harness every suite in this crate drives the emulator through.
use super::emulator::{
    error_code, flag, id_of, list, number, timestamp, wait_for_terminal, wait_until_started,
    Emulator, ABSENT_ID, RUN_TIME_LOWER_BOUND_SECONDS, UNBOUNDED_SQL, WAIT_SECONDS,
};

// ---------------------------------------------------------------------------
// The rows a statement produced
// ---------------------------------------------------------------------------

/// One row of the result-fidelity table: the SQL submitted, and what the engine
/// must be shown to have produced for it.
struct ResultCase {
    label: &'static str,
    /// Statements submitted (and completed) before the one under test.
    setup: &'static [&'static str],
    sql: &'static str,
    /// `HasResultSet` the statement must record.
    has_result_set: bool,
    /// `ResultRows`: rows returned, or rows affected for a DML statement.
    result_rows: i64,
    /// The exact `Records` `GetStatementResult` must return.
    records: fn() -> Value,
}

/// The rows, counters and record shape a statement reports are the ones the SQL
/// engine produced for the SQL that was submitted — across literal projections,
/// every member of the `Field` union, DDL, DML and queries over stored rows.
#[tokio::test]
async fn reported_rows_are_what_the_engine_produced_for_the_submitted_sql() {
    let cases = [
        ResultCase {
            label: "a one-column literal projection",
            setup: &[],
            sql: "SELECT 1",
            has_result_set: true,
            result_rows: 1,
            records: || json!([[{"longValue": 1}]]),
        },
        ResultCase {
            label: "two columns of different types",
            setup: &[],
            sql: "SELECT 2 AS a, 'x' AS b",
            has_result_set: true,
            result_rows: 1,
            records: || json!([[{"longValue": 2}, {"stringValue": "x"}]]),
        },
        ResultCase {
            label: "a floating point literal",
            setup: &[],
            sql: "SELECT 1.5 AS ratio",
            has_result_set: true,
            result_rows: 1,
            records: || json!([[{"doubleValue": 1.5}]]),
        },
        ResultCase {
            label: "a NULL value",
            setup: &[],
            sql: "SELECT NULL AS blank",
            has_result_set: true,
            result_rows: 1,
            records: || json!([[{"isNull": true}]]),
        },
        ResultCase {
            label: "a binary value",
            setup: &[],
            sql: "SELECT x'6162' AS raw",
            has_result_set: true,
            result_rows: 1,
            records: || json!([[{"blobValue": "YWI="}]]),
        },
        ResultCase {
            label: "a boolean column",
            setup: &[
                "CREATE TABLE flags (flag BOOLEAN)",
                "INSERT INTO flags (flag) VALUES (1), (0)",
            ],
            sql: "SELECT flag FROM flags ORDER BY flag DESC",
            has_result_set: true,
            result_rows: 2,
            records: || json!([[{"booleanValue": true}], [{"booleanValue": false}]]),
        },
        ResultCase {
            label: "a DDL statement",
            setup: &[],
            sql: "CREATE TABLE t (id INTEGER, label VARCHAR(16))",
            has_result_set: false,
            result_rows: 0,
            records: || json!([]),
        },
        ResultCase {
            label: "a DML statement affecting several rows",
            setup: &["CREATE TABLE t (id INTEGER, label VARCHAR(16))"],
            sql: "INSERT INTO t (id, label) VALUES (1, 'a'), (2, 'b'), (3, 'c')",
            has_result_set: false,
            result_rows: 3,
            records: || json!([]),
        },
        ResultCase {
            label: "a query over rows a previous statement stored",
            setup: &[
                "CREATE TABLE t (id INTEGER, label VARCHAR(16))",
                "INSERT INTO t (id, label) VALUES (1, 'a'), (2, 'b')",
            ],
            sql: "SELECT id, label FROM t ORDER BY id",
            has_result_set: true,
            result_rows: 2,
            records: || {
                json!([
                    [{"longValue": 1}, {"stringValue": "a"}],
                    [{"longValue": 2}, {"stringValue": "b"}]
                ])
            },
        },
        ResultCase {
            label: "an aggregate computed over stored rows",
            setup: &[
                "CREATE TABLE t (id INTEGER)",
                "INSERT INTO t (id) VALUES (4), (5), (6)",
            ],
            sql: "SELECT COUNT(*) AS n, SUM(id) AS total FROM t",
            has_result_set: true,
            result_rows: 1,
            records: || json!([[{"longValue": 3}, {"longValue": 15}]]),
        },
        ResultCase {
            label: "an UPDATE that touches a subset of the rows",
            setup: &[
                "CREATE TABLE t (id INTEGER)",
                "INSERT INTO t (id) VALUES (1), (2), (3), (4)",
            ],
            sql: "UPDATE t SET id = id + 10 WHERE id > 2",
            has_result_set: false,
            result_rows: 2,
            records: || json!([]),
        },
        ResultCase {
            label: "a DELETE that removes no rows",
            setup: &["CREATE TABLE t (id INTEGER)"],
            sql: "DELETE FROM t WHERE id = 99",
            has_result_set: false,
            result_rows: 0,
            records: || json!([]),
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        for sql in case.setup {
            emulator.run(sql).await;
        }

        let submitted = emulator
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": case.sql,
            }))
            .await;
        let id = id_of(&submitted);

        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["QueryString"], case.sql,
            "case {}: DescribeStatement must report the SQL that was submitted, got {described}",
            case.label
        );
        assert_eq!(
            described["Status"], "FINISHED",
            "case {}: expected FINISHED, got {described}",
            case.label
        );
        assert_eq!(
            flag(&described, "HasResultSet"),
            case.has_result_set,
            "case {}: expected HasResultSet {}, got {described}",
            case.label,
            case.has_result_set
        );
        assert_eq!(
            number(&described, "ResultRows"),
            case.result_rows,
            "case {}: expected ResultRows {}, got {described}",
            case.label,
            case.result_rows
        );
        assert_eq!(
            described["Database"], "dev",
            "case {}: expected the submitted database, got {described}",
            case.label
        );
        assert_eq!(
            described["ClusterIdentifier"], "my-cluster",
            "case {}: expected the submitted cluster, got {described}",
            case.label
        );

        // A statement that produced a result set is read back through
        // `GetStatementResult`; one that produced none has no RESULT to read,
        // so the read is refused rather than answered with an empty page.
        if !case.has_result_set {
            let (status, refused) = emulator
                .call("GetStatementResult", &json!({"Id": id}))
                .await;
            assert_eq!(
                status, 400,
                "case {}: a statement with no result set has no result to fetch, got {status} \
                 {refused}",
                case.label
            );
            assert_eq!(
                error_code(&refused),
                "ResourceNotFoundException",
                "case {}: expected ResourceNotFoundException, got {refused}",
                case.label
            );
            continue;
        }

        let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;
        let expected_records = (case.records)();
        assert_eq!(
            fetched["Records"], expected_records,
            "case {}: expected Records {expected_records}, got {fetched}",
            case.label
        );
        let expected_total = case.result_rows;
        assert_eq!(
            number(&fetched, "TotalNumRows"),
            expected_total,
            "case {}: expected TotalNumRows {expected_total}, got {fetched}",
            case.label
        );
        let column_count = list(&fetched, "ColumnMetadata").len();
        let row_width = list(&fetched, "Records")
            .first()
            .map_or(column_count, |row| row.as_array().map_or(0, Vec::len));
        assert_eq!(
            column_count, row_width,
            "case {}: ColumnMetadata must have one entry per column of a record, got {fetched}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// Column metadata
// ---------------------------------------------------------------------------

/// One row of the column-metadata table.
struct TypeCase {
    /// The type the column is declared with.
    declared: &'static str,
    /// The `typeName` the column must report.
    type_name: &'static str,
    /// `length` (and therefore `precision`).
    length: i64,
    /// `scale`.
    scale: i64,
    /// `isSigned`.
    signed: bool,
    /// `isCaseSensitive`.
    case_sensitive: bool,
}

/// `ColumnMetadata.typeName` reports the database-specific name of the column's
/// declared type, with the declared size carried through to `length`,
/// `precision` and `scale`.
///
/// The expectations come from the Amazon Redshift *Data types* documentation
/// (<https://docs.aws.amazon.com/redshift/latest/dg/c_Supported_data_types.html>),
/// whose per-type rows give each keyword's internal name.
#[tokio::test]
async fn column_metadata_reports_the_declared_type_of_each_column() {
    let cases = [
        TypeCase { declared: "SMALLINT", type_name: "int2", length: 0, scale: 0, signed: true, case_sensitive: false },
        TypeCase { declared: "INTEGER", type_name: "int4", length: 0, scale: 0, signed: true, case_sensitive: false },
        TypeCase { declared: "INT", type_name: "int4", length: 0, scale: 0, signed: true, case_sensitive: false },
        TypeCase { declared: "BIGINT", type_name: "int8", length: 0, scale: 0, signed: true, case_sensitive: false },
        TypeCase { declared: "DECIMAL(10,2)", type_name: "numeric", length: 10, scale: 2, signed: true, case_sensitive: false },
        TypeCase { declared: "NUMERIC(38,10)", type_name: "numeric", length: 38, scale: 10, signed: true, case_sensitive: false },
        TypeCase { declared: "REAL", type_name: "float4", length: 0, scale: 0, signed: true, case_sensitive: false },
        TypeCase { declared: "DOUBLE PRECISION", type_name: "float8", length: 0, scale: 0, signed: true, case_sensitive: false },
        TypeCase { declared: "BOOLEAN", type_name: "bool", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "CHAR(4)", type_name: "bpchar", length: 4, scale: 0, signed: false, case_sensitive: true },
        TypeCase { declared: "VARCHAR(16)", type_name: "varchar", length: 16, scale: 0, signed: false, case_sensitive: true },
        TypeCase { declared: "VARCHAR(65535)", type_name: "varchar", length: 65535, scale: 0, signed: false, case_sensitive: true },
        TypeCase { declared: "TEXT", type_name: "varchar", length: 0, scale: 0, signed: false, case_sensitive: true },
        TypeCase { declared: "DATE", type_name: "date", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "TIME", type_name: "time", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "TIMETZ", type_name: "timetz", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "TIMESTAMP", type_name: "timestamp", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "TIMESTAMPTZ", type_name: "timestamptz", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "VARBYTE", type_name: "varbyte", length: 0, scale: 0, signed: false, case_sensitive: false },
        TypeCase { declared: "SUPER", type_name: "super", length: 0, scale: 0, signed: false, case_sensitive: true },
        // A keyword outside the mapping keeps its own spelling rather than being
        // reported as some other type.
        TypeCase { declared: "GEOMETRY", type_name: "geometry", length: 0, scale: 0, signed: false, case_sensitive: false },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        emulator
            .run(&format!("CREATE TABLE t (col {})", case.declared))
            .await;
        let id = emulator.run("SELECT col FROM t").await;
        let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;

        let columns = list(&fetched, "ColumnMetadata");
        assert_eq!(
            columns.len(),
            1,
            "case {}: expected one column, got {fetched}",
            case.declared
        );
        let column = &columns[0];
        assert_eq!(
            column["name"], "col",
            "case {}: expected the column's name, got {column}",
            case.declared
        );
        assert_eq!(
            column["label"], "col",
            "case {}: expected the column's label, got {column}",
            case.declared
        );
        assert_eq!(
            column["typeName"], case.type_name,
            "case {}: expected typeName {}, got {column}",
            case.declared, case.type_name
        );
        assert_eq!(
            number(column, "length"),
            case.length,
            "case {}: expected length {}, got {column}",
            case.declared,
            case.length
        );
        assert_eq!(
            number(column, "precision"),
            case.length,
            "case {}: precision must carry the declared size, got {column}",
            case.declared
        );
        assert_eq!(
            number(column, "scale"),
            case.scale,
            "case {}: expected scale {}, got {column}",
            case.declared,
            case.scale
        );
        assert_eq!(
            flag(column, "isSigned"),
            case.signed,
            "case {}: expected isSigned {}, got {column}",
            case.declared,
            case.signed
        );
        assert_eq!(
            flag(column, "isCaseSensitive"),
            case.case_sensitive,
            "case {}: expected isCaseSensitive {}, got {column}",
            case.declared,
            case.case_sensitive
        );
        assert!(
            !flag(column, "isCurrency"),
            "case {}: no Redshift type is a currency type, got {column}",
            case.declared
        );
    }
}

/// The published range of Redshift's 4-byte `INTEGER` (`int4`), from the Amazon
/// Redshift *Data types* documentation
/// (<https://docs.aws.amazon.com/redshift/latest/dg/c_Supported_data_types.html>):
/// "INTEGER, INT, INT4 — Signed four-byte integer — -2147483648 to +2147483647".
/// A value past either end needs `BIGINT` (`int8`), whose range is
/// -9223372036854775808 to +9223372036854775807.
const INT4_MIN: i64 = -2_147_483_648;
const INT4_MAX: i64 = 2_147_483_647;

/// A column with no declared type — an expression such as `SELECT 1` — reports
/// the type of the value the engine produced for it, and an INTEGER value takes
/// the NARROWEST Redshift integer type whose published range holds it: `int4`
/// inside the `INTEGER` range, `int8` past it. That is how a Redshift
/// (PostgreSQL-derived) integer constant is typed, which is why a bare integer
/// literal is an `int4` column and not an `int8` one.
///
/// The rows span what the rule generalises over: the value's type (integer, real,
/// text, binary, NULL), its sign and magnitude (both published `int4` bounds and
/// one past each), and whether the column is a literal or a computed expression.
#[tokio::test]
async fn an_expression_column_reports_the_type_of_the_value_it_produced() {
    let cases: Vec<(String, Option<&str>)> = vec![
        ("SELECT 1 AS c".to_string(), Some("int4")),
        ("SELECT 0 AS c".to_string(), Some("int4")),
        ("SELECT -7 AS c".to_string(), Some("int4")),
        // Each published bound of the INTEGER range, and one step past it.
        (format!("SELECT {INT4_MAX} AS c"), Some("int4")),
        (format!("SELECT {INT4_MIN} AS c"), Some("int4")),
        (format!("SELECT {} AS c", INT4_MAX + 1), Some("int8")),
        (format!("SELECT {} AS c", INT4_MIN - 1), Some("int8")),
        (format!("SELECT {} AS c", i64::MAX), Some("int8")),
        // A computed integer expression is typed by the value it produces, the
        // same way a literal is — including one whose operands fit `int4` and
        // whose result does not.
        ("SELECT 2 + 3 AS c".to_string(), Some("int4")),
        (
            format!("SELECT {INT4_MAX} + 1 AS c"),
            Some("int8"),
        ),
        ("SELECT 1.25 AS c".to_string(), Some("float8")),
        ("SELECT -1.25 AS c".to_string(), Some("float8")),
        ("SELECT 'text' AS c".to_string(), Some("varchar")),
        ("SELECT x'00ff' AS c".to_string(), Some("varbyte")),
        // A NULL says nothing about the column's type, so none is reported
        // rather than one being guessed.
        ("SELECT NULL AS c".to_string(), None),
    ];

    let emulator = Emulator::new();
    for (sql, expected) in cases {
        let id = emulator.run(&sql).await;
        let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;
        let columns = list(&fetched, "ColumnMetadata");
        assert_eq!(
            columns.len(),
            1,
            "case {sql}: expected one column, got {fetched}"
        );
        let column = &columns[0];
        match expected {
            Some(type_name) => assert_eq!(
                column["typeName"], type_name,
                "case {sql}: expected typeName {type_name}, got {column}"
            ),
            None => assert!(
                column["typeName"].is_null(),
                "case {sql}: a column of NULLs reports no typeName, got {column}"
            ),
        }
        // Every Redshift integer type is signed, and none of them is a character
        // type — the flags a client reads beside `typeName`.
        if matches!(expected, Some("int4" | "int8")) {
            assert!(
                flag(column, "isSigned"),
                "case {sql}: an integer column is signed, got {column}"
            );
            assert!(
                !flag(column, "isCaseSensitive"),
                "case {sql}: an integer column is not case-sensitive, got {column}"
            );
        }
    }
}

/// An inferred column type has to hold EVERY value the column carried, so a
/// column whose values are all inside the `INTEGER` range is `int4` and one
/// single value past that range makes the whole column `int8` — whichever row it
/// arrives in.
#[tokio::test]
async fn an_inferred_integer_column_widens_to_hold_every_value_it_carries() {
    let past_int4 = INT4_MAX + 1;
    let cases: Vec<(String, &str)> = vec![
        (
            format!("SELECT 1 AS c UNION ALL SELECT 2 UNION ALL SELECT {INT4_MAX}"),
            "int4",
        ),
        // The wide value arrives last: the first row's type must not be the
        // column's answer.
        (
            format!("SELECT 1 AS c UNION ALL SELECT {past_int4}"),
            "int8",
        ),
        // ... and first.
        (
            format!("SELECT {past_int4} AS c UNION ALL SELECT 1"),
            "int8",
        ),
        // A leading NULL says nothing, so the type comes from the values after it.
        (
            format!("SELECT NULL AS c UNION ALL SELECT {past_int4}"),
            "int8",
        ),
        ("SELECT NULL AS c UNION ALL SELECT 1".to_string(), "int4"),
    ];

    let emulator = Emulator::new();
    for (sql, expected) in cases {
        let id = emulator.run(&sql).await;
        let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;
        let columns = list(&fetched, "ColumnMetadata");
        assert_eq!(
            columns.len(),
            1,
            "case {sql}: expected one column, got {fetched}"
        );
        assert_eq!(
            columns[0]["typeName"], expected,
            "case {sql}: expected typeName {expected}, got {fetched}"
        );
    }
}

// ---------------------------------------------------------------------------
// The asynchronous lifecycle
// ---------------------------------------------------------------------------

/// A submitted statement walks the lifecycle its execution really walks —
/// `SUBMITTED` when it is answered, `STARTED` while the engine runs it — and
/// reports its terminal status once that execution has finished; nothing about
/// the result is reported before then.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_submitted_statement_reaches_its_terminal_status_when_its_execution_does() {
    let emulator = Emulator::new();
    // A trivial statement reports its outcome as soon as that outcome exists:
    // the status follows the ENGINE, so a caller polling right after the submit
    // reads what the statement really did rather than waiting out a span the
    // work did not take.
    let quick = emulator
        .ok(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": "SELECT 1",
            }),
        )
        .await;
    let quick_id = id_of(&quick);
    assert!(
        ["SUBMITTED", "PICKED", "STARTED", "FINISHED"]
            .contains(&quick["Status"].as_str().unwrap_or("")),
        "a statement nobody waited for is answered with the state its execution has reached, \
         which is one of the states of a statement that was really submitted: {quick}"
    );
    assert!(
        quick["CreatedAt"].is_number(),
        "ExecuteStatement must report CreatedAt: {quick}"
    );
    assert!(
        is_uuid_shaped(&quick_id),
        "the minted Id must match the modelled UUID pattern, got {quick_id:?}"
    );
    // Polled rather than waited out: the status follows the ENGINE, so what the
    // read below reports is the outcome the execution really reached and not
    // whatever the statement had got to after a span of this test's choosing.
    let finished = wait_for_terminal(&emulator, "a statement nobody waited for", &quick_id).await;
    assert_eq!(
        finished["Status"], "FINISHED",
        "a statement whose execution finished reports its terminal status: {finished}"
    );
    assert!(
        flag(&finished, "HasResultSet"),
        "SELECT 1 produces a result set: {finished}"
    );
    assert_eq!(
        number(&finished, "ResultRows"),
        1,
        "SELECT 1 produces one row: {finished}"
    );
    assert!(
        timestamp(&finished, "UpdatedAt") - timestamp(&finished, "CreatedAt")
            >= RUN_TIME_LOWER_BOUND_SECONDS,
        "UpdatedAt is the instant the execution finished, so it is at or after \
         CreatedAt: {finished}"
    );
    assert!(
        number(&finished, "Duration") > 0,
        "a statement that ran reports the time it took: {finished}"
    );
    assert_eq!(
        number(&finished, "RedshiftPid"),
        number(&quick, "RedshiftPid"),
        "the connection's process id must not change between reads: {finished}"
    );

    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": quick_id}))
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"longValue": 1}]]),
        "expected the row the statement produced, got {fetched}"
    );

    // And a statement that runs until a bound of the engine stops it is
    // observable WHILE it runs: it reports STARTED — the query run has started —
    // nothing about its result, and no rows to fetch. What keeps that window
    // open is the engine's own execution bound, so these reads never race the
    // end of the statement's work.
    let submitted = emulator
        .ok(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": UNBOUNDED_SQL,
                "StatementName": "boot",
            }),
        )
        .await;
    let id = id_of(&submitted);
    assert!(
        ["SUBMITTED", "PICKED", "STARTED"].contains(&submitted["Status"].as_str().unwrap_or("")),
        "a statement nobody waited for is answered with the state it has reached, which is not \
         a terminal one: {submitted}"
    );

    let pending = wait_until_started(&emulator, "a statement that takes real time", &id).await;
    assert_eq!(
        pending["Status"], "STARTED",
        "a statement whose execution the engine is running is STARTED: {pending}"
    );
    assert_eq!(
        number(&pending, "Duration"),
        0,
        "a running statement has not run for any time yet: {pending}"
    );
    assert_eq!(
        number(&pending, "ResultRows"),
        0,
        "a running statement reports no rows yet: {pending}"
    );
    assert!(
        pending.get("HasResultSet").is_none(),
        "whether a running statement has a result set is not known yet: {pending}"
    );

    let (status, refused) = emulator
        .call("GetStatementResult", &json!({"Id": id}))
        .await;
    assert_eq!(
        status, 400,
        "a running statement has no rows to fetch: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "expected ValidationException, got {refused}"
    );

    // Stopped rather than left running: the statement was submitted to be
    // observed while it ran, and it has been.
    emulator.ok("CancelStatement", &json!({"Id": id})).await;
    wait_for_terminal(&emulator, "the statement that was observed running", &id).await;
}

/// Whether a string matches the modelled `UUID` pattern
/// `^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$`.
fn is_uuid_shaped(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    let widths = [8_usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups.iter().zip(widths.iter()).all(|(group, width)| {
            group.len() == *width
                && group
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        })
}

/// A statement the engine rejects ends FAILED, carrying the engine's own error,
/// and reports the modelled "null" counters rather than inventing a result.
#[tokio::test]
async fn a_statement_the_engine_rejects_ends_failed_with_the_engine_s_error() {
    let cases = [
        "SELECT * FROM no_such_table",
        "THIS IS NOT SQL",
        "INSERT INTO no_such_table (a) VALUES (1)",
    ];

    let emulator = Emulator::new();
    for sql in cases {
        let submitted = emulator
            .ok(
                "ExecuteStatement",
                &json!({
                    "WorkgroupName": "my-workgroup",
                    "Database": "dev",
                    "Sql": sql,
                    "WaitTimeSeconds": WAIT_SECONDS,
                }),
            )
            .await;
        let id = id_of(&submitted);
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["Status"], "FAILED",
            "case {sql}: the engine rejected this statement, got {described}"
        );
        assert!(
            described["Error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "case {sql}: a failed statement must carry the engine's error, got {described}"
        );
        assert_eq!(
            number(&described, "ResultRows"),
            -1,
            "case {sql}: a failed statement has no row count, got {described}"
        );
        assert_eq!(
            number(&described, "ResultSize"),
            -1,
            "case {sql}: a failed statement has no result size, got {described}"
        );

        let (status, refused) = emulator
            .call("GetStatementResult", &json!({"Id": id}))
            .await;
        assert_eq!(
            status, 400,
            "case {sql}: a failed statement has no result to fetch, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {sql}: expected ResourceNotFoundException, got {refused}"
        );
    }
}

// ---------------------------------------------------------------------------
// CancelStatement
// ---------------------------------------------------------------------------

/// A running statement is cancellable and the new status is visible to the next
/// read; a statement that has already stopped is not reported as cancelled.
#[tokio::test]
async fn cancelling_a_running_statement_is_visible_to_the_next_read() {
    let emulator = Emulator::new();
    let submitted = emulator
        .ok(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": "SELECT 1",
            }),
        )
        .await;
    let id = id_of(&submitted);

    // A trivial statement's execution finishes in about the time this call takes
    // to arrive, so either outcome is possible and both are the same rule — a
    // RUNNING statement is cancellable, a STOPPED one is not. The branch asserts
    // whichever one the engine produced, and never treats an already-terminal
    // statement as a cancellation.
    let (cancel_status, cancelled) = emulator.call("CancelStatement", &json!({"Id": id})).await;
    if cancel_status == 200 {
        assert_eq!(
            cancelled["Status"],
            json!(true),
            "cancelling a running statement reports true: {cancelled}"
        );

        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["Status"], "ABORTED",
            "the cancellation must be visible to the next read: {described}"
        );
        assert!(
            !flag(&described, "HasResultSet"),
            "an aborted statement has no result set: {described}"
        );

        // Cancelling it again is not a second cancellation.
        let (status, refused) = emulator.call("CancelStatement", &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "an aborted statement is not running: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "expected ValidationException, got {refused}"
        );
    } else {
        assert_eq!(
            cancel_status, 400,
            "a statement whose execution already finished is refused, not cancelled: {cancelled}"
        );
        assert_eq!(
            error_code(&cancelled),
            "ValidationException",
            "expected ValidationException, got {cancelled}"
        );
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "a refused cancellation must leave the terminal status alone: {described}"
        );
    }

    // Nor is a statement whose execution already finished.
    let finished = emulator.run("SELECT 2").await;
    let (status, refused) = emulator
        .call("CancelStatement", &json!({"Id": finished}))
        .await;
    assert_eq!(
        status, 400,
        "a finished statement must not be reported as cancelled: {refused}"
    );
}

// ---------------------------------------------------------------------------
// ListStatements
// ---------------------------------------------------------------------------

/// `ListStatements` enumerates newest submission first, caps each page at
/// `MaxResults`, and resumes a page exactly where the previous one stopped.
#[tokio::test]
async fn list_statements_pages_newest_first_without_gaps_or_repeats() {
    let emulator = Emulator::new();
    let mut submitted = Vec::new();
    for index in 0..7 {
        submitted.push(emulator.run(&format!("SELECT {index}")).await);
    }
    submitted.reverse();

    // Every page size from one to more than the whole set must enumerate the
    // same statements in the same order.
    for page_size in [1_usize, 2, 3, 7, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= 20,
                "page size {page_size}: paging did not terminate after {pages} pages"
            );
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let page = emulator.ok("ListStatements", &request).await;
            let statements = list(&page, "Statements");
            assert!(
                statements.len() <= page_size,
                "page size {page_size}: a page must not exceed MaxResults, got {page}"
            );
            for statement in &statements {
                seen.push(
                    statement["Id"]
                        .as_str()
                        .unwrap_or_else(|| panic!("a Statements entry carries no Id: {statement}"))
                        .to_string(),
                );
            }
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, submitted,
            "page size {page_size}: expected every statement newest-first exactly once"
        );
    }

    // The entries are built from the stored statement, not from a canned body.
    let page = emulator.ok("ListStatements", &json!({"MaxResults": 1})).await;
    let newest = &list(&page, "Statements")[0];
    assert_eq!(
        newest["QueryString"], "SELECT 6",
        "the newest statement's SQL must come from state: {newest}"
    );
    assert_eq!(
        newest["Status"], "FINISHED",
        "the newest statement's status must come from state: {newest}"
    );
    assert_eq!(
        newest["IsBatchStatement"],
        json!(false),
        "ExecuteStatement submits a single statement: {newest}"
    );
    assert_eq!(
        newest["ResultFormat"], "json",
        "the default result format is JSON, reported in the spelling the service \
         answers with: {newest}"
    );
}

/// One row of the filter table: a `ListStatements` request, and the statement
/// names it must select.
struct FilterCase {
    label: &'static str,
    request: fn() -> Value,
    expected: &'static [&'static str],
}

/// Each `ListStatements` filter selects exactly the statements that match it.
#[tokio::test]
async fn list_statements_honours_each_filter() {
    let emulator = Emulator::new();

    // Three statements on a cluster, two on a serverless workgroup, one on a
    // second database, and one left running so a status filter has something to
    // separate.
    for (name, sql) in [("alpha-one", "SELECT 1"), ("alpha-two", "SELECT 2"), ("beta-one", "SELECT 3")] {
        emulator
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": sql,
                "StatementName": name,
            }))
            .await;
    }
    for (name, sql) in [("gamma-one", "SELECT 4"), ("gamma-two", "SELECT 5")] {
        emulator
            .execute_and_wait(&json!({
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "Sql": sql,
                "StatementName": name,
            }))
            .await;
    }
    emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "reporting",
            "DbUser": "awsuser",
            "Sql": "SELECT 6",
            "StatementName": "delta-one",
        }))
        .await;

    let cases = [
        FilterCase {
            label: "no filter",
            request: || json!({}),
            expected: &[
                "delta-one",
                "gamma-two",
                "gamma-one",
                "beta-one",
                "alpha-two",
                "alpha-one",
            ],
        },
        FilterCase {
            label: "a statement-name prefix",
            request: || json!({"StatementName": "alpha"}),
            expected: &["alpha-two", "alpha-one"],
        },
        FilterCase {
            label: "a statement-name prefix matching one entry",
            request: || json!({"StatementName": "beta"}),
            expected: &["beta-one"],
        },
        FilterCase {
            label: "a case-sensitive statement-name prefix",
            request: || json!({"StatementName": "ALPHA"}),
            expected: &[],
        },
        FilterCase {
            label: "a full statement name",
            request: || json!({"StatementName": "gamma-two"}),
            expected: &["gamma-two"],
        },
        FilterCase {
            label: "the SUBMITTED status, which every statement here has left",
            request: || json!({"Status": "SUBMITTED"}),
            expected: &[],
        },
        FilterCase {
            label: "the FINISHED status",
            request: || json!({"Status": "FINISHED"}),
            expected: &[
                "delta-one",
                "gamma-two",
                "gamma-one",
                "beta-one",
                "alpha-two",
                "alpha-one",
            ],
        },
        FilterCase {
            label: "a status no statement reached",
            request: || json!({"Status": "ABORTED"}),
            expected: &[],
        },
        FilterCase {
            label: "a status every statement in this table has already left",
            request: || json!({"Status": "STARTED"}),
            expected: &[],
        },
        FilterCase {
            label: "the ALL status, which includes every status",
            request: || json!({"Status": "ALL"}),
            expected: &[
                "delta-one",
                "gamma-two",
                "gamma-one",
                "beta-one",
                "alpha-two",
                "alpha-one",
            ],
        },
        FilterCase {
            label: "a database",
            request: || json!({"Database": "reporting"}),
            expected: &["delta-one"],
        },
        FilterCase {
            label: "a cluster identifier",
            request: || json!({"ClusterIdentifier": "my-cluster"}),
            expected: &["delta-one", "beta-one", "alpha-two", "alpha-one"],
        },
        FilterCase {
            label: "a workgroup name",
            request: || json!({"WorkgroupName": "my-workgroup"}),
            expected: &["gamma-two", "gamma-one"],
        },
        FilterCase {
            label: "a cluster that ran nothing",
            request: || json!({"ClusterIdentifier": "other-cluster"}),
            expected: &[],
        },
        // `RoleLevel` chooses between every statement run by the caller's IAM
        // role (true) and only those run in the current IAM session (false).
        // This emulator serves one caller in one session, so both selections are
        // the SAME, non-empty set — asserted here rather than only claimed in
        // the handler's comment.
        FilterCase {
            label: "RoleLevel true, which selects every statement of the caller's role",
            request: || json!({"RoleLevel": true}),
            expected: &[
                "delta-one",
                "gamma-two",
                "gamma-one",
                "beta-one",
                "alpha-two",
                "alpha-one",
            ],
        },
        FilterCase {
            label: "RoleLevel false, which selects the current session's statements",
            request: || json!({"RoleLevel": false}),
            expected: &[
                "delta-one",
                "gamma-two",
                "gamma-one",
                "beta-one",
                "alpha-two",
                "alpha-one",
            ],
        },
        FilterCase {
            label: "RoleLevel true beside another filter",
            request: || json!({"RoleLevel": true, "ClusterIdentifier": "my-cluster"}),
            expected: &["delta-one", "beta-one", "alpha-two", "alpha-one"],
        },
        FilterCase {
            label: "RoleLevel false beside another filter",
            request: || json!({"RoleLevel": false, "ClusterIdentifier": "my-cluster"}),
            expected: &["delta-one", "beta-one", "alpha-two", "alpha-one"],
        },
        FilterCase {
            label: "two filters together",
            request: || json!({"ClusterIdentifier": "my-cluster", "Status": "FINISHED"}),
            expected: &["delta-one", "beta-one", "alpha-two", "alpha-one"],
        },
        FilterCase {
            label: "two filters that cannot both hold",
            request: || json!({"WorkgroupName": "my-workgroup", "Database": "reporting"}),
            expected: &[],
        },
    ];

    for case in &cases {
        let page = emulator.ok("ListStatements", &(case.request)()).await;
        let names: Vec<String> = list(&page, "Statements")
            .iter()
            .map(|statement| {
                statement["StatementName"]
                    .as_str()
                    .unwrap_or("<unnamed>")
                    .to_string()
            })
            .collect();
        assert_eq!(
            names, case.expected,
            "case {}: expected {:?}, got {page}",
            case.label, case.expected
        );
    }
}

/// The `Status` filter separates a statement that is still running from one that
/// has finished.
///
/// Kept apart from the filter table above because an execution takes real time:
/// what each page is required to hold is therefore stated as an invariant over
/// the statuses the page itself reports, not as one fixed list.
#[tokio::test]
async fn the_status_filter_separates_a_running_statement_from_a_finished_one() {
    let emulator = Emulator::new();
    emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sql": "SELECT 1",
            "StatementName": "finished-one",
        }))
        .await;
    emulator
        .ok(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": "SELECT 2",
                "StatementName": "running-one",
            }),
        )
        .await;

    // Each filtered page must hold exactly the statements carrying that status.
    // The waited-for statement is FINISHED for good, so it must never match
    // SUBMITTED and must always match FINISHED; the statement nobody waited for
    // is SUBMITTED until its own execution reports, so it may appear under
    // either, and the page itself says which.
    for filter in ["SUBMITTED", "FINISHED"] {
        let response = emulator
            .ok("ListStatements", &json!({"Status": filter}))
            .await;
        let page = list(&response, "Statements");
        for statement in &page {
            assert_eq!(
                statement["Status"], filter,
                "the {filter} filter returned a statement with another status: {statement}"
            );
        }
        let names: Vec<String> = page
            .iter()
            .map(|statement| {
                statement["StatementName"]
                    .as_str()
                    .unwrap_or("<unnamed>")
                    .to_string()
            })
            .collect();
        match filter {
            "SUBMITTED" => assert!(
                !names.contains(&"finished-one".to_string())
                    && names.len() <= 1
                    && names.iter().all(|name| name == "running-one"),
                "the SUBMITTED filter must match at most the statement nobody waited for, got {names:?}"
            ),
            _ => assert!(
                names.contains(&"finished-one".to_string()),
                "the FINISHED filter must match the statement that was waited for, got {names:?}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// GetStatementResult pagination
// ---------------------------------------------------------------------------
/// `GetStatementResult` returns every row of the result set exactly once across
/// its pages, carries the column metadata on each page, reports the full row
/// count on each page and stops issuing a token when nothing remains.
#[tokio::test]
async fn get_statement_result_pages_the_rows_it_produced() {
    const ROWS: i64 = 2_500;

    let emulator = Emulator::new();
    emulator.run("CREATE TABLE wide (id BIGINT)").await;
    emulator
        .run(&format!(
            "WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM counter \
             WHERE x < {ROWS}) INSERT INTO wide (id) SELECT x FROM counter"
        ))
        .await;
    let id = emulator.run("SELECT id FROM wide ORDER BY id").await;

    let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    assert_eq!(
        number(&described, "ResultRows"),
        ROWS,
        "the statement returned {ROWS} rows: {described}"
    );
    assert!(
        number(&described, "ResultSize") > 0,
        "a result set has a size in bytes: {described}"
    );

    let mut collected: Vec<i64> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(pages <= 10, "paging did not terminate after {pages} pages");
        let mut request = json!({"Id": id});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("GetStatementResult", &request).await;
        assert_eq!(
            number(&page, "TotalNumRows"),
            ROWS,
            "page {pages}: TotalNumRows is the whole result set, got {page}"
        );
        let columns = list(&page, "ColumnMetadata");
        assert_eq!(
            columns.len(),
            1,
            "page {pages}: every page carries the column metadata, got {page}"
        );
        assert_eq!(
            columns[0]["typeName"], "int8",
            "page {pages}: expected the declared BIGINT type, got {page}"
        );
        for record in list(&page, "Records") {
            let row = record
                .as_array()
                .unwrap_or_else(|| panic!("a record is not a list: {record}"));
            assert_eq!(row.len(), 1, "a record must have one field per column: {record}");
            collected.push(
                row[0]["longValue"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("expected a longValue, got {record}")),
            );
        }
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }

    assert!(
        pages > 1,
        "{ROWS} rows must not fit in a single page, got {pages}"
    );
    assert_eq!(
        collected,
        (1..=ROWS).collect::<Vec<i64>>(),
        "every row must be returned exactly once, in order"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Every operation that takes an `Id` refuses an `Id` no statement carries with
/// the missing-resource error at HTTP 400, in the service's fixed words.
#[tokio::test]
async fn an_id_no_statement_carries_is_refused() {
    let emulator = Emulator::new();
    // A statement exists, so the refusal is about this `Id` and not about empty
    // state.
    emulator.run("SELECT 1").await;

    for operation in ["DescribeStatement", "GetStatementResult", "CancelStatement"] {
        let (status, response) = emulator.call(operation, &json!({"Id": ABSENT_ID})).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected 400 for an unknown Id, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {operation}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["Message"].as_str(),
            Some("Query does not exist."),
            "case {operation}: expected the service's fixed wording, got {response}"
        );
    }
}

/// A `NextToken` neither paginated operation issued is refused, rather than
/// silently restarting from the beginning or skipping rows.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let id = emulator.run("SELECT 1").await;

    let cases = [
        ("ListStatements", "not-hexadecimal"),
        ("ListStatements", ""),
        // Well-formed hex, but not a cursor.
        ("ListStatements", "6162636465"),
        // A cursor of the OTHER operation's kind.
        ("ListStatements", "726f77733a35"),
        // The right kind with a position that is not a number.
        ("ListStatements", "73746174656d656e74733a616263"),
        ("GetStatementResult", "not-hexadecimal"),
        ("GetStatementResult", "6162636465"),
        // A `statements` cursor handed to the row-paging operation.
        ("GetStatementResult", "73746174656d656e74733a35"),
        // A row position past the end of this statement's single row.
        ("GetStatementResult", "726f77733a393939"),
    ];

    for (operation, token) in cases {
        let mut request = json!({"NextToken": token});
        if operation == "GetStatementResult" {
            request["Id"] = json!(id);
        }
        let (status, response) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {operation} {token:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {operation} {token:?}: expected ValidationException, got {response}"
        );
    }

    // A row cursor is bound to the statement it was issued for, so the same
    // offset in another statement's result set is refused rather than served.
    let other = emulator.run("SELECT 2").await;
    let cursor_for_the_first = hex::encode(format!("rows:{id}:0"));
    let (status, response) = emulator
        .call(
            "GetStatementResult",
            &json!({"Id": other, "NextToken": cursor_for_the_first}),
        )
        .await;
    assert_eq!(
        status, 400,
        "a row cursor issued for another statement must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response}"
    );

    // The refusal rule does not depend on the statement having produced rows,
    // but a statement with no result set has no result to page at all: the read
    // is refused as a missing result before any token is considered.
    let no_result_set = emulator.run("CREATE TABLE token_probe (a INTEGER)").await;
    for token in [
        "not-hexadecimal",
        "6162636465",
        // The right kind, but a position past this statement's (empty) result.
        "726f77733a39",
    ] {
        let (status, response) = emulator
            .call(
                "GetStatementResult",
                &json!({"Id": no_result_set, "NextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {token:?} on a statement with no result set: expected 400, got {status} \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {token:?} on a statement with no result set: expected \
             ResourceNotFoundException, got {response}"
        );
    }

    // And so is its first page, with no token at all.
    let (status, response) = emulator
        .call("GetStatementResult", &json!({"Id": no_result_set}))
        .await;
    assert_eq!(
        status, 400,
        "a statement with no result set has no page to serve, got {status} {response}"
    );
}

/// `ExecuteStatement` refuses a request that does not name exactly one data
/// warehouse, one database and one authorization method.
#[tokio::test]
async fn execute_statement_refuses_a_request_that_names_no_single_target() {
    let emulator = Emulator::new();
    let cases: [(&str, Value); 6] = [
        ("neither a cluster nor a workgroup", json!({"Database": "dev", "Sql": "SELECT 1"})),
        (
            "both a cluster and a workgroup",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
                "Sql": "SELECT 1",
            }),
        ),
        (
            "no database",
            json!({"ClusterIdentifier": "my-cluster", "DbUser": "awsuser", "Sql": "SELECT 1"}),
        ),
        (
            "a blank database",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "   ",
                "DbUser": "awsuser",
                "Sql": "SELECT 1",
            }),
        ),
        (
            "two authorization methods at once",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
                "Sql": "SELECT 1",
            }),
        ),
        (
            "no SQL statement",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": "   ",
            }),
        ),
    ];

    for (label, request) in &cases {
        let (status, response) = emulator.call("ExecuteStatement", request).await;
        assert_eq!(
            status, 400,
            "case {label}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {label}: expected ValidationException, got {response}"
        );
    }

    // And nothing was stored for any of them.
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert!(
        list(&page, "Statements").is_empty(),
        "a refused request must store no statement, got {page}"
    );

    // The session-reuse form carries its connection on the SESSION, so a
    // `SessionId` that names no live session names no warehouse and no database
    // either: a missing resource, not a 400 that would blame the request, and it
    // stores nothing.
    let (status, response) = emulator
        .call(
            "ExecuteStatement",
            &json!({
                "SessionId": "3dc6a4b5-5e2b-4b5a-9f1d-6b6d2b6c7a8e",
                "Sql": "SELECT 1",
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "a SessionId naming no live session is a missing resource: got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {response}"
    );
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert!(
        list(&page, "Statements").is_empty(),
        "a refused request must store no statement, got {page}"
    );
}

/// Each authorization combination the documentation names is accepted, and the
/// statement reports the credentials it was submitted with.
#[tokio::test]
async fn each_documented_authorization_combination_is_accepted() {
    let emulator = Emulator::new();
    let cases: [(&str, Value); 4] = [
        (
            "a cluster with a database user",
            json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"}),
        ),
        (
            "a cluster with a secret",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
            }),
        ),
        (
            "a cluster with an IAM identity",
            json!({"ClusterIdentifier": "my-cluster", "Database": "dev"}),
        ),
        (
            "a serverless workgroup",
            json!({"WorkgroupName": "my-workgroup", "Database": "dev"}),
        ),
    ];

    for (label, connection) in &cases {
        let mut request = connection.clone();
        request["Sql"] = json!("SELECT 1");
        let submitted = emulator.execute_and_wait(&request).await;
        let described = emulator
            .ok("DescribeStatement", &json!({"Id": id_of(&submitted)}))
            .await;

        for member in ["ClusterIdentifier", "WorkgroupName", "DbUser", "SecretArn"] {
            assert_eq!(
                described.get(member).cloned().unwrap_or(Value::Null),
                connection.get(member).cloned().unwrap_or(Value::Null),
                "case {label}: {member} must be reported as submitted, got {described}"
            );
        }
        assert_eq!(
            described["ExecutionMode"], "AUTO_COMMIT",
            "case {label}: a single statement is committed on its own, got {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// Parameters, sessions, idempotency and database scoping
// ---------------------------------------------------------------------------

/// Named `Parameters` are bound by the engine, and the statement reports the
/// parameters it was submitted with.
#[tokio::test]
async fn named_parameters_are_bound_by_the_engine() {
    let emulator = Emulator::new();
    emulator
        .run("CREATE TABLE people (id INTEGER, name VARCHAR(32))")
        .await;
    emulator
        .run("INSERT INTO people (id, name) VALUES (1, 'ada'), (2, 'alan'), (3, 'grace')")
        .await;

    let cases: [(&str, Value, Value); 3] = [
        (
            "one parameter",
            json!([{"name": "wanted", "value": "alan"}]),
            json!([[{"longValue": 2}, {"stringValue": "alan"}]]),
        ),
        (
            "a parameter matching nothing",
            json!([{"name": "wanted", "value": "nobody"}]),
            json!([]),
        ),
        (
            "a parameter matching another row",
            json!([{"name": "wanted", "value": "grace"}]),
            json!([[{"longValue": 3}, {"stringValue": "grace"}]]),
        ),
    ];

    for (label, parameters, expected) in &cases {
        let submitted = emulator
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "Sql": "SELECT id, name FROM people WHERE name = :wanted",
                "Parameters": parameters,
            }))
            .await;
        let id = id_of(&submitted);
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["QueryParameters"], *parameters,
            "case {label}: the submitted parameters must be reported, got {described}"
        );
        let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;
        assert_eq!(
            fetched["Records"], *expected,
            "case {label}: expected {expected}, got {fetched}"
        );
    }
}

/// Repeating an `ExecuteStatement` with the same `ClientToken` answers with the
/// statement the first call submitted instead of submitting a second one.
#[tokio::test]
async fn a_repeated_client_token_replays_the_first_statement() {
    let emulator = Emulator::new();
    let request = json!({
        "ClusterIdentifier": "my-cluster",
        "Database": "dev",
        "DbUser": "awsuser",
        "Sql": "SELECT 1",
        "ClientToken": "token-one",
    });

    let first = emulator.ok("ExecuteStatement", &request).await;
    let second = emulator.ok("ExecuteStatement", &request).await;
    assert_eq!(
        id_of(&first),
        id_of(&second),
        "the same ClientToken must answer with the same statement: {first} vs {second}"
    );

    let mut other = request.clone();
    other["ClientToken"] = json!("token-two");
    let third = emulator.ok("ExecuteStatement", &other).await;
    assert_ne!(
        id_of(&first),
        id_of(&third),
        "a different ClientToken submits a new statement: {first} vs {third}"
    );

    let page = emulator.ok("ListStatements", &json!({})).await;
    assert_eq!(
        list(&page, "Statements").len(),
        2,
        "two tokens submitted two statements, not three: {page}"
    );
}

/// A session identifier the caller supplies is carried on the statement, and
/// asking to keep a session alive mints one.
#[tokio::test]
async fn a_session_identifier_is_carried_on_the_statement() {
    let emulator = Emulator::new();

    let minted = emulator
        .execute_and_wait(&json!({
            "WorkgroupName": "my-workgroup",
            "Database": "dev",
            "Sql": "SELECT 1",
            "SessionKeepAliveSeconds": 60,
        }))
        .await;
    let session = minted["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("asking to keep a session alive must mint one: {minted}"))
        .to_string();
    assert!(
        is_uuid_shaped(&session),
        "a minted SessionId must match the modelled UUID pattern, got {session:?}"
    );

    // A statement submitted into a session names the SESSION: the connection
    // comes off the session, which is why the reused statement still reports the
    // workgroup and database the session was opened with.
    let reused = emulator
        .execute_and_wait(&json!({
            "Sql": "SELECT 2",
            "SessionId": session,
        }))
        .await;
    assert_eq!(
        reused["SessionId"], session,
        "a supplied SessionId must be carried through: {reused}"
    );
    assert_eq!(
        (
            reused["WorkgroupName"].as_str(),
            reused["Database"].as_str()
        ),
        (Some("my-workgroup"), Some("dev")),
        "a statement in a session runs on the connection the session was opened with: {reused}"
    );
    let described = emulator
        .ok("DescribeStatement", &json!({"Id": id_of(&reused)}))
        .await;
    assert_eq!(
        described["SessionId"], session,
        "DescribeStatement must report the session: {described}"
    );

    let without = emulator
        .execute_and_wait(&json!({
            "WorkgroupName": "my-workgroup",
            "Database": "dev",
            "Sql": "SELECT 3",
        }))
        .await;
    assert!(
        without.get("SessionId").is_none(),
        "a statement that asked for no session reports none: {without}"
    );
}

/// Rows a statement stores belong to that data warehouse's database and to no
/// other: the same SQL succeeds where the table was created and fails anywhere
/// else.
#[tokio::test]
async fn a_table_belongs_to_the_database_it_was_created_in() {
    let emulator = Emulator::new();
    emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sql": "CREATE TABLE scoped (id INTEGER)",
        }))
        .await;
    emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "dev",
            "DbUser": "awsuser",
            "Sql": "INSERT INTO scoped (id) VALUES (42)",
        }))
        .await;

    let cases: [(&str, Value, bool); 4] = [
        (
            "the database the table was created in",
            json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"}),
            true,
        ),
        (
            "the same cluster, another database",
            json!({"ClusterIdentifier": "my-cluster", "Database": "reporting", "DbUser": "awsuser"}),
            false,
        ),
        (
            "another cluster",
            json!({"ClusterIdentifier": "other-cluster", "Database": "dev", "DbUser": "awsuser"}),
            false,
        ),
        (
            "a serverless workgroup",
            json!({"WorkgroupName": "my-workgroup", "Database": "dev"}),
            false,
        ),
    ];

    for (label, connection, expect_rows) in &cases {
        let mut request = connection.clone();
        request["Sql"] = json!("SELECT id FROM scoped");
        let submitted = emulator.ok("ExecuteStatement", &{
            let mut waited = request.clone();
            waited["WaitTimeSeconds"] = json!(WAIT_SECONDS);
            waited
        })
        .await;
        let id = id_of(&submitted);
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;

        if *expect_rows {
            assert_eq!(
                described["Status"], "FINISHED",
                "case {label}: the table exists here, got {described}"
            );
            let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;
            assert_eq!(
                fetched["Records"],
                json!([[{"longValue": 42}]]),
                "case {label}: expected the stored row, got {fetched}"
            );
        } else {
            assert_eq!(
                described["Status"], "FAILED",
                "case {label}: the table does not exist here, got {described}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Engine confinement and bounds
// ---------------------------------------------------------------------------

/// The SQL engine is a real engine, so the statements that would let a caller
/// reach the HOST rather than the database are refused before they run: the
/// statement ends FAILED, no file appears on the host, and the engine's message
/// carries no host path back to the caller.
#[tokio::test]
async fn statements_that_would_reach_the_host_are_refused() {
    let emulator = Emulator::new();

    // A path the emulator must never create. Each case gets its own, so a case
    // that did create a file names itself.
    let target = |case: &str| {
        std::env::temp_dir().join(format!(
            "redshift-data-emulator-escape-{case}-{}.db",
            std::process::id()
        ))
    };

    /// How one escape case builds its SQL from the host path it tries to reach.
    type BuildSql = fn(&str) -> String;

    let cases: [(&str, BuildSql); 8] = [
        ("attach", |path| {
            format!("ATTACH DATABASE '{path}' AS escaped")
        }),
        ("attach-uri", |path| {
            format!("ATTACH DATABASE 'file:{path}?mode=rwc' AS escaped")
        }),
        ("vacuum-into", |path| format!("VACUUM INTO '{path}'")),
        ("detach", |_| "DETACH DATABASE main".to_string()),
        // A `PRAGMA` is how the confinement set at open would be lifted, so no
        // submitted statement may issue one.
        ("pragma-temp-store", |_| {
            "PRAGMA temp_store = FILE".to_string()
        }),
        ("pragma-page-count", |_| {
            "PRAGMA max_page_count = 1000000000".to_string()
        }),
        ("load-extension", |path| {
            format!("SELECT load_extension('{path}')")
        }),
        ("virtual-table", |path| {
            format!("CREATE VIRTUAL TABLE escaped USING csv(filename='{path}')")
        }),
    ];

    for (case, build) in cases {
        let path = target(case);
        let path_text = path.to_string_lossy().into_owned();
        let sql = build(&path_text);

        let submitted = emulator
            .ok(
                "ExecuteStatement",
                &json!({
                    "ClusterIdentifier": "my-cluster",
                    "Database": "dev",
                    "DbUser": "awsuser",
                    "Sql": sql,
                    "WaitTimeSeconds": WAIT_SECONDS,
                }),
            )
            .await;
        let described = emulator
            .ok("DescribeStatement", &json!({"Id": id_of(&submitted)}))
            .await;
        assert_eq!(
            described["Status"], "FAILED",
            "case {case}: {sql} must be refused by the engine, got {described}"
        );
        let error = described["Error"].as_str().unwrap_or_default();
        assert!(
            !error.contains(&path_text),
            "case {case}: the engine's message must not carry a host path, got {error:?}"
        );
        assert!(
            !path.exists(),
            "case {case}: {sql} created {}",
            path.display()
        );
    }
}

/// A statement that would run without end, or that exceeds the engine's size and
/// depth bounds, ends FAILED rather than consuming the emulator.
#[tokio::test]
async fn statements_outside_the_engine_s_bounds_are_interrupted_or_refused() {
    let emulator = Emulator::new();

    // Nested 300 deep, well past the expression-depth bound.
    let deep = format!("SELECT {}1{}", "(1 + ".repeat(300), ")".repeat(300));
    // Chained 80 ways, past the compound-select bound.
    let compound = (0..80)
        .map(|_| "SELECT 1")
        .collect::<Vec<&str>>()
        .join(" UNION ALL ");

    let cases: [(&str, String); 7] = [
        (
            // Counts for ever: it emits one row, so neither the row cap nor the
            // byte cap can stop it — only the step budget and the deadline can.
            "an unbounded recursive query",
            UNBOUNDED_SQL.to_string(),
        ),
        (
            "a value larger than the engine's length limit",
            "SELECT randomblob(4000000)".to_string(),
        ),
        (
            // Past what one result set may retain, so it must FAIL rather than
            // report a truncated result as a complete one.
            "a result set past the retained-result bound",
            format!(
                "WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM counter \
                 WHERE x < {}) SELECT x FROM counter",
                MAX_RESULT_ROWS + 1
            ),
        ),
        (
            // The engine's storage bound on the `main` schema. Each row carries
            // a value just under the engine's own per-value ceiling, so the
            // bound is reached in few rows rather than many.
            "a table larger than the database may hold",
            "CREATE TABLE bulk AS WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL \
             SELECT x + 1 FROM counter WHERE x < 400) SELECT x, randomblob(900000) AS pad \
             FROM counter"
                .to_string(),
        ),
        (
            // The same bound on the `temp` schema, which `temp_store = MEMORY`
            // puts on the process heap and which a `main`-only bound would miss.
            "a temporary table larger than the database may hold",
            "CREATE TEMP TABLE bulk AS WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL \
             SELECT x + 1 FROM counter WHERE x < 400) SELECT x, randomblob(900000) AS pad \
             FROM counter"
                .to_string(),
        ),
        ("an expression nested past the depth limit", deep),
        ("a compound select past its limit", compound),
    ];

    for (label, sql) in &cases {
        let submitted = emulator
            .ok(
                "ExecuteStatement",
                &json!({
                    "ClusterIdentifier": "my-cluster",
                    "Database": "dev",
                    "DbUser": "awsuser",
                    "Sql": sql,
                    "WaitTimeSeconds": WAIT_SECONDS,
                }),
            )
            .await;
        // The verdict is the ENGINE's and arrives on the engine's own bound, so
        // it is polled for rather than read once and assumed to have arrived
        // inside this request's wait.
        let described = wait_for_terminal(&emulator, label, &id_of(&submitted)).await;
        assert_eq!(
            described["Status"], "FAILED",
            "case {label}: must end FAILED rather than run unbounded, got {described}"
        );
        assert!(
            described["Error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "case {label}: the engine's own error must say why, got {described}"
        );
        if *label == "an unbounded recursive query" {
            // This one has to be stopped by the step budget / deadline rather
            // than refused at prepare time, so pin which bound fired.
            assert!(
                described["Error"]
                    .as_str()
                    .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
                "case {label}: must be stopped by the execution bound, got {described}"
            );
        }
    }

    // And the emulator is still serving after all of them.
    let id = emulator.run("SELECT 1").await;
    let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;
    assert_eq!(
        fetched["Records"],
        json!([[{"longValue": 1}]]),
        "the service must still serve after a bounded statement was interrupted: {fetched}"
    );
}

/// The statement history is bounded: the oldest submissions are evicted, so a
/// caller repeating `ExecuteStatement` cannot make the emulator retain result
/// sets without limit.
#[tokio::test]
async fn the_statement_history_is_bounded() {
    let emulator = Emulator::new();
    let overflow = 10_usize;

    let mut ids = Vec::new();
    for index in 0..(MAX_RETAINED_STATEMENTS + overflow) {
        ids.push(emulator.run(&format!("SELECT {index}")).await);
    }

    // The oldest submissions past the bound are gone.
    for (index, id) in ids.iter().take(overflow).enumerate() {
        let (status, response) = emulator.call("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "statement {index} is past the retention bound and must be evicted, got {response}"
        );
    }
    // The newest are all still readable.
    for (index, id) in ids.iter().skip(overflow).enumerate() {
        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["QueryString"],
            format!("SELECT {}", index + overflow),
            "a retained statement must still report its own SQL, got {described}"
        );
    }

    // And the history holds exactly the bound, not more.
    let mut retained = 0_usize;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({"MaxResults": 100});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("ListStatements", &request).await;
        retained += list(&page, "Statements").len();
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        retained, MAX_RETAINED_STATEMENTS,
        "the history must hold exactly the retention bound"
    );
}

/// The number of databases a caller can make the emulator open is bounded: the
/// key is caller-chosen, so a request naming one past the bound is refused
/// rather than served by opening another engine.
#[tokio::test]
async fn the_number_of_open_databases_is_bounded() {
    let emulator = Emulator::new();

    for index in 0..MAX_OPEN_DATABASES {
        emulator
            .execute_and_wait(&json!({
                "ClusterIdentifier": "my-cluster",
                "Database": format!("database{index}"),
                "DbUser": "awsuser",
                "Sql": "SELECT 1",
            }))
            .await;
    }

    let (status, refused) = emulator
        .call(
            "ExecuteStatement",
            &json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "one-too-many",
                "DbUser": "awsuser",
                "Sql": "SELECT 1",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a database past the bound must be refused, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ActiveStatementsExceededException",
        "a capacity limit is reported as a capacity error, not as a bad request: {refused}"
    );

    // A database already open still works, so the bound refuses only what is new.
    let served = emulator
        .execute_and_wait(&json!({
            "ClusterIdentifier": "my-cluster",
            "Database": "database0",
            "DbUser": "awsuser",
            "Sql": "SELECT 1",
        }))
        .await;
    assert_eq!(
        served["Status"], "FINISHED",
        "an already-open database must keep serving: {served}"
    );
}

// ---------------------------------------------------------------------------
// Operations that are still scaffolded
// ---------------------------------------------------------------------------

/// The operations this slice does not implement still answer 501, so a client
/// can tell "not written yet" from an empty success.
#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_not_implemented() {
    let emulator = Emulator::new();
    let cases: [(&str, Value); 2] = [
        (
            "ListSchemas",
            json!({"ClusterIdentifier": "my-cluster", "Database": "dev"}),
        ),
        (
            "ListTables",
            json!({"ClusterIdentifier": "my-cluster", "Database": "dev"}),
        ),
    ];

    for (operation, request) in &cases {
        let (status, response) = emulator.call(operation, request).await;
        assert_eq!(
            status, 501,
            "case {operation}: expected 501 NotImplemented, got {status} {response}"
        );
    }
}
