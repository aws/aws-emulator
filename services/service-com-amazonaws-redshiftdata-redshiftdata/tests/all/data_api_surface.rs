// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Data API's response and validation surface over a
//! provisioned cluster and a serverless workgroup, driven over the wire: every
//! call below is a real `POST /` with `X-Amz-Target: RedshiftData.<Operation>`
//! dispatched through the router, so what is asserted is the emulator's HTTP
//! behaviour and not a handler's return value.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! case:
//!
//! * a value is rendered under the `Field` member its COLUMN's type calls for:
//!   an exact-numeric column as `stringValue` at the column's declared scale,
//!   an approximate-numeric one as `doubleValue`, an integer as `longValue`, a
//!   boolean as `booleanValue` and a character or temporal column as
//!   `stringValue`;
//! * `ColumnMetadata.precision` is the declared width of a non-numeric column
//!   and the declared precision of an exact-numeric one;
//! * a statement with no result to fetch is refused as a missing RESULT,
//!   while a query that matched nothing is served as the empty result set it
//!   produced;
//! * a statement record's `Id` is the `Id` of the SUBMISSION it belongs to, and
//!   a batch's results are readable only per statement of the batch;
//! * `ListSessions` reports the sessions this service holds, honours each
//!   filter and refuses `SessionId` combined with any other one;
//! * `DescribeTable` reports the columns a table of a database declares.

use serde_json::{json, Value};

// The harness every suite in this crate drives the emulator through.
use super::emulator::{
    error_code, id_of, list, number, wait_for_terminal, Emulator, SLOW_SQL, WAIT_SECONDS,
};

/// The connection every statement in this suite runs through unless it names
/// another: the provisioned cluster the issue was recorded against.
fn cluster() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

// ---------------------------------------------------------------------------
// Which `Field` member a value is rendered under
// ---------------------------------------------------------------------------

/// One row of the field-rendering table: a column's declared type, the SQL
/// literal stored in it, and the `Field` the read must return.
struct FieldCase {
    label: &'static str,
    declared: &'static str,
    stored: &'static str,
    field: fn() -> Value,
}

/// A value is reported under the `Field` member its COLUMN's type calls for.
///
/// An exact-numeric column (`DECIMAL`/`NUMERIC`, the types the Amazon Redshift
/// *Numeric types* documentation separates from the approximate `REAL` and
/// `DOUBLE PRECISION`) is rendered as `stringValue` carrying the value at the
/// column's declared scale: a JSON number cannot carry that scale, and
/// rendering one would round-trip the value through binary floating point.
#[tokio::test]
async fn a_value_is_rendered_under_the_field_member_its_column_type_calls_for() {
    let cases = [
        FieldCase {
            label: "a DECIMAL whose value needs its declared scale",
            declared: "DECIMAL(10,2)",
            stored: "10.50",
            field: || json!({"stringValue": "10.50"}),
        },
        FieldCase {
            label: "a negative DECIMAL",
            declared: "DECIMAL(10,2)",
            stored: "-3.25",
            field: || json!({"stringValue": "-3.25"}),
        },
        FieldCase {
            label: "a DECIMAL whose value is a whole number",
            declared: "DECIMAL(10,2)",
            stored: "10.00",
            field: || json!({"stringValue": "10.00"}),
        },
        FieldCase {
            label: "a NUMERIC with a wider scale than the value needs",
            declared: "NUMERIC(12,4)",
            stored: "1.5",
            field: || json!({"stringValue": "1.5000"}),
        },
        FieldCase {
            label: "a NUMERIC declared with no scale at all",
            declared: "NUMERIC(8,0)",
            stored: "42",
            field: || json!({"stringValue": "42"}),
        },
        FieldCase {
            label: "a DECIMAL past what a double holds exactly",
            declared: "DECIMAL(20,0)",
            stored: "123456789012345678",
            field: || json!({"stringValue": "123456789012345678"}),
        },
        FieldCase {
            label: "a NULL in an exact-numeric column",
            declared: "DECIMAL(10,2)",
            stored: "NULL",
            field: || json!({"isNull": true}),
        },
        // The approximate numeric types keep the floating-point member: they
        // declare no scale, and a double is what the column stores.
        FieldCase {
            label: "a REAL column",
            declared: "REAL",
            stored: "1.5",
            field: || json!({"doubleValue": 1.5}),
        },
        FieldCase {
            label: "a DOUBLE PRECISION column",
            declared: "DOUBLE PRECISION",
            stored: "2.25",
            field: || json!({"doubleValue": 2.25}),
        },
        // And every other family keeps the member it already had.
        FieldCase {
            label: "an INTEGER column",
            declared: "INTEGER",
            stored: "7",
            field: || json!({"longValue": 7}),
        },
        FieldCase {
            label: "a BIGINT column",
            declared: "BIGINT",
            stored: "9007199254740993",
            field: || json!({"longValue": 9_007_199_254_740_993_i64}),
        },
        FieldCase {
            label: "a BOOLEAN column",
            declared: "BOOLEAN",
            stored: "1",
            field: || json!({"booleanValue": true}),
        },
        FieldCase {
            label: "a VARCHAR column",
            declared: "VARCHAR(32)",
            stored: "'alpha'",
            field: || json!({"stringValue": "alpha"}),
        },
        FieldCase {
            label: "a DATE column",
            declared: "DATE",
            stored: "'2026-09-22'",
            field: || json!({"stringValue": "2026-09-22"}),
        },
        FieldCase {
            label: "a TIMESTAMP column",
            declared: "TIMESTAMP",
            stored: "'2026-09-22 10:11:12'",
            field: || json!({"stringValue": "2026-09-22 10:11:12"}),
        },
        FieldCase {
            label: "a TIMESTAMPTZ column",
            declared: "TIMESTAMPTZ",
            stored: "'2026-01-02 03:04:05+00'",
            field: || json!({"stringValue": "2026-01-02 03:04:05+00"}),
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        emulator
            .run(&format!("CREATE TABLE t (col {})", case.declared))
            .await;
        emulator
            .run(&format!("INSERT INTO t (col) VALUES ({})", case.stored))
            .await;
        let id = emulator.run("SELECT col FROM t").await;
        let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;

        let expected = json!([[(case.field)()]]);
        assert_eq!(
            fetched["Records"], expected,
            "case {}: expected Records {expected}, got {fetched}",
            case.label
        );
    }
}

/// The same rule holds for every column of ONE row read at once, which is how a
/// client reads a wide table: each cell takes the member its own column calls
/// for.
#[tokio::test]
async fn every_column_of_one_row_takes_its_own_member() {
    let emulator = Emulator::new();
    emulator
        .run(
            "CREATE TABLE wide (id INT, name VARCHAR(32), amt DECIMAL(10,2), flag BOOLEAN, \
             ts TIMESTAMP, ratio DOUBLE PRECISION)",
        )
        .await;
    emulator
        .run(
            "INSERT INTO wide (id, name, amt, flag, ts, ratio) VALUES \
             (1, 'alpha', 10.50, 1, '2026-09-22 10:11:12', 0.5), \
             (2, NULL, -3.25, 0, '2026-01-02 03:04:05', 1.25)",
        )
        .await;
    let id = emulator
        .run("SELECT id, name, amt, flag, ts, ratio FROM wide ORDER BY id")
        .await;
    let fetched = emulator.ok("GetStatementResult", &json!({"Id": id})).await;

    let expected = json!([
        [
            {"longValue": 1},
            {"stringValue": "alpha"},
            {"stringValue": "10.50"},
            {"booleanValue": true},
            {"stringValue": "2026-09-22 10:11:12"},
            {"doubleValue": 0.5},
        ],
        [
            {"longValue": 2},
            {"isNull": true},
            {"stringValue": "-3.25"},
            {"booleanValue": false},
            {"stringValue": "2026-01-02 03:04:05"},
            {"doubleValue": 1.25},
        ],
    ]);
    assert_eq!(
        fetched["Records"], expected,
        "expected Records {expected}, got {fetched}"
    );
}

// ---------------------------------------------------------------------------
// ColumnMetadata.precision
// ---------------------------------------------------------------------------

/// One row of the precision table: a column's declared type and the
/// `precision`, `scale` and `length` it must report.
struct PrecisionCase {
    declared: &'static str,
    precision: i64,
    scale: i64,
}

/// `ColumnMetadata.precision` is "the precision value of a decimal number
/// column, or the column length for a non-numeric column", so it carries the
/// size the column was DECLARED with rather than the modelled default of 0 —
/// nothing else in the response carries the declared width.
#[tokio::test]
async fn column_metadata_precision_is_the_declared_size_of_the_column() {
    let cases = [
        PrecisionCase { declared: "VARCHAR(32)", precision: 32, scale: 0 },
        PrecisionCase { declared: "VARCHAR(1)", precision: 1, scale: 0 },
        PrecisionCase { declared: "VARCHAR(65535)", precision: 65535, scale: 0 },
        PrecisionCase { declared: "CHAR(8)", precision: 8, scale: 0 },
        PrecisionCase { declared: "DECIMAL(10,2)", precision: 10, scale: 2 },
        PrecisionCase { declared: "NUMERIC(38,10)", precision: 38, scale: 10 },
        // A type that declares no size at all reports the modelled default,
        // which is then the truth about it rather than a dropped value.
        PrecisionCase { declared: "INTEGER", precision: 0, scale: 0 },
        PrecisionCase { declared: "BOOLEAN", precision: 0, scale: 0 },
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
            number(column, "precision"),
            case.precision,
            "case {}: expected precision {}, got {column}",
            case.declared,
            case.precision
        );
        assert_eq!(
            number(column, "scale"),
            case.scale,
            "case {}: expected scale {}, got {column}",
            case.declared,
            case.scale
        );
        assert_eq!(
            number(column, "length"),
            case.precision,
            "case {}: a declared size is reported as both length and precision, got {column}",
            case.declared
        );
    }
}

// ---------------------------------------------------------------------------
// Reading the result of a statement that produced none
// ---------------------------------------------------------------------------

/// A statement that produced NO RESULT is refused as a missing result resource
/// (`ResourceNotFoundException`, at the client-input status the service answers
/// that refusal with) — whatever terminal status it reached —
/// while a query that produced a result set is served, even an empty one: "no
/// result set" and "an empty result set" are different answers.
#[tokio::test]
async fn a_statement_with_no_result_is_refused_as_a_missing_result() {
    // Each row: the SQL to submit, and whether it produces a result set.
    let cases: [(&str, &str, bool); 6] = [
        ("a FAILED statement", "SELECT id FROM no_such_table", false),
        ("a FINISHED DDL statement", "CREATE TABLE made (id INTEGER)", false),
        (
            "a FINISHED INSERT",
            "INSERT INTO probe (id) VALUES (1), (2)",
            false,
        ),
        ("a FINISHED UPDATE", "UPDATE probe SET id = id + 1", false),
        ("a query that matched nothing", "SELECT id FROM probe WHERE id = 99", true),
        ("a query that matched rows", "SELECT id FROM probe", true),
    ];

    let emulator = Emulator::new();
    emulator.run("CREATE TABLE probe (id INTEGER)").await;

    for (label, sql, has_result_set) in &cases {
        let mut request = cluster();
        request["Sql"] = json!(sql);
        request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        let submitted = emulator.ok("ExecuteStatement", &request).await;
        let id = id_of(&submitted);

        let (status, response) = emulator
            .call("GetStatementResult", &json!({"Id": id}))
            .await;
        if *has_result_set {
            assert_eq!(
                status, 200,
                "case {label}: a statement that produced a result set is served, got {status} \
                 {response}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label}: a statement with no result has no result to read, got {status} \
                 {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {label}: expected ResourceNotFoundException, got {response}"
            );
            // The refused read leaves the statement's own record untouched: it
            // is still describable, and still reports what it did.
            let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
            assert!(
                described["Status"].is_string(),
                "case {label}: the statement must still describe itself, got {described}"
            );
        }
    }

    // An ABORTED statement reaches the same refusal: it was stopped, so it has
    // no result either.
    let mut request = cluster();
    request["Sql"] = json!("SELECT id FROM probe");
    let submitted = emulator.ok("ExecuteStatement", &request).await;
    let aborted = id_of(&submitted);
    let cancelled = emulator
        .ok("CancelStatement", &json!({"Id": aborted}))
        .await;
    assert!(
        cancelled["Status"].as_bool().unwrap_or(false),
        "the statement must still have been cancellable: {cancelled}"
    );
    let (status, response) = emulator
        .call("GetStatementResult", &json!({"Id": aborted}))
        .await;
    assert_eq!(
        status, 400,
        "an aborted statement has no result to read, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {response}"
    );
}

// ---------------------------------------------------------------------------
// A batch's identity and where its results live
// ---------------------------------------------------------------------------

/// A statement record reports the `Id` of the SUBMISSION it belongs to: for one
/// statement of a batch, reached through the `<batch id>:<n>` address, that is
/// the batch's own `Id`, while `SubStatements` keeps the positional address.
/// And a batch's results are readable only per statement of the batch — reading
/// the batch's own `Id` is refused.
#[tokio::test]
async fn a_batch_statement_reports_the_batch_id_and_its_results_live_per_statement() {
    let cases: [(&str, &[&str]); 3] = [
        ("one statement", &["SELECT 1"]),
        ("three queries", &["SELECT 1", "SELECT 2", "SELECT 3"]),
        (
            "DDL, DML and a query",
            &[
                "CREATE TABLE b (id INTEGER)",
                "INSERT INTO b (id) VALUES (1)",
                "SELECT id FROM b",
            ],
        ),
    ];

    for (label, sqls) in &cases {
        let emulator = Emulator::new();
        let mut request = cluster();
        request["Sqls"] = json!(sqls);
        request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
        let submitted = emulator.ok("BatchExecuteStatement", &request).await;
        let batch_id = id_of(&submitted);

        let described = emulator
            .ok("DescribeStatement", &json!({"Id": batch_id}))
            .await;
        assert_eq!(
            described["Id"], json!(batch_id),
            "case {label}: a batch reports its own Id, got {described}"
        );
        let subs = list(&described, "SubStatements");
        assert_eq!(
            subs.len(),
            sqls.len(),
            "case {label}: one SubStatements entry per statement, got {described}"
        );

        for (index, sub) in subs.iter().enumerate() {
            let position = index + 1;
            let address = format!("{batch_id}:{position}");
            assert_eq!(
                sub["Id"], json!(address),
                "case {label}: SubStatements carries the positional address, got {sub}"
            );
            let described_sub = emulator
                .ok("DescribeStatement", &json!({"Id": address}))
                .await;
            assert_eq!(
                described_sub["Id"], json!(batch_id),
                "case {label}: statement {position} must report the batch's Id, got \
                 {described_sub}"
            );
            assert_eq!(
                described_sub["QueryString"], json!(sqls[index]),
                "case {label}: statement {position} must report its own text, got \
                 {described_sub}"
            );
        }

        // The batch's own Id addresses no result: a batch is not a query.
        let (status, refused) = emulator
            .call("GetStatementResult", &json!({"Id": batch_id}))
            .await;
        assert_eq!(
            status, 400,
            "case {label}: a batch's own Id has no result page, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {label}: expected ValidationException, got {refused}"
        );

        // While the statement of the batch that produced rows serves them.
        let last = format!("{batch_id}:{}", sqls.len());
        let fetched = emulator.ok("GetStatementResult", &json!({"Id": last})).await;
        assert_eq!(
            number(&fetched, "TotalNumRows"),
            1,
            "case {label}: the batch's last statement returned one row, got {fetched}"
        );
    }
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// Open a session on `connection` by asking to keep one alive, and return its
/// `SessionId`.
async fn open_session(emulator: &Emulator, connection: &Value, keep_alive_seconds: i64) -> String {
    let mut request = connection.clone();
    request["Sql"] = json!("SELECT 1");
    request["SessionKeepAliveSeconds"] = json!(keep_alive_seconds);
    let submitted = emulator.execute_and_wait(&request).await;
    submitted["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("asking to keep a session alive must mint one: {submitted}"))
        .to_string()
}

/// `ListSessions` reports the sessions this service holds, with the metadata of
/// each one, and honours each filter the operation documents.
#[tokio::test]
async fn list_sessions_reports_the_sessions_it_holds_and_honours_each_filter() {
    let emulator = Emulator::new();
    let on_cluster = open_session(&emulator, &cluster(), 90).await;
    let on_other_database = open_session(
        &emulator,
        &json!({"ClusterIdentifier": "my-cluster", "Database": "reporting", "DbUser": "awsuser"}),
        120,
    )
    .await;
    let on_workgroup = open_session(
        &emulator,
        &json!({"WorkgroupName": "my-workgroup", "Database": "dev"}),
        60,
    )
    .await;

    // Every session, newest first.
    let page = emulator.ok("ListSessions", &json!({})).await;
    let listed: Vec<String> = list(&page, "Sessions")
        .iter()
        .map(|session| session["SessionId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![
            on_workgroup.clone(),
            on_other_database.clone(),
            on_cluster.clone()
        ],
        "every live session must be listed, newest first, got {page}"
    );

    // The metadata of one session, read through `SessionId`.
    let single = emulator
        .ok("ListSessions", &json!({"SessionId": on_cluster}))
        .await;
    let sessions = list(&single, "Sessions");
    assert_eq!(
        sessions.len(),
        1,
        "SessionId asks for one session's metadata, got {single}"
    );
    let session = &sessions[0];
    for (member, expected) in [
        ("SessionId", json!(on_cluster)),
        ("Status", json!("AVAILABLE")),
        ("ClusterIdentifier", json!("my-cluster")),
        ("Database", json!("dev")),
        ("DbUser", json!("awsuser")),
        ("SessionAliveSeconds", json!(90)),
    ] {
        assert_eq!(
            session[member], expected,
            "expected {member} {expected}, got {session}"
        );
    }
    assert!(
        session.get("WorkgroupName").is_none(),
        "a session on a cluster reports no workgroup, got {session}"
    );
    assert!(
        session.get("CurrentStatementId").is_none(),
        "an AVAILABLE session is running no statement, got {session}"
    );
    assert!(
        session["CreatedAt"].is_number() && session["SessionTtl"].is_number(),
        "a session reports when it was created and when it expires, got {session}"
    );

    // Each set filter, over the sessions above.
    let filters: [(&str, Value, Vec<&String>); 5] = [
        (
            "the cluster",
            json!({"ClusterIdentifier": "my-cluster"}),
            vec![&on_other_database, &on_cluster],
        ),
        (
            "the workgroup",
            json!({"WorkgroupName": "my-workgroup"}),
            vec![&on_workgroup],
        ),
        (
            "the database",
            json!({"Database": "dev"}),
            vec![&on_workgroup, &on_cluster],
        ),
        (
            "an AVAILABLE status",
            json!({"Status": "AVAILABLE"}),
            vec![&on_workgroup, &on_other_database, &on_cluster],
        ),
        // Nothing is closed yet, so the CLOSED selection is empty rather than
        // the whole set.
        ("a CLOSED status", json!({"Status": "CLOSED"}), vec![]),
    ];
    for (label, filter, expected) in &filters {
        let page = emulator.ok("ListSessions", filter).await;
        let listed: Vec<String> = list(&page, "Sessions")
            .iter()
            .map(|session| session["SessionId"].as_str().unwrap_or_default().to_string())
            .collect();
        let expected: Vec<String> = expected.iter().map(|id| (*id).clone()).collect();
        assert_eq!(
            listed, expected,
            "case {label}: expected {expected:?}, got {page}"
        );
    }

    // And the page boundary resumes exactly where the previous page stopped.
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut request = json!({"MaxResults": 1});
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("ListSessions", &request).await;
        let sessions = list(&page, "Sessions");
        assert!(
            sessions.len() <= 1,
            "page {pages}: MaxResults 1 holds at most one session, got {page}"
        );
        seen.extend(
            sessions
                .iter()
                .map(|session| session["SessionId"].as_str().unwrap_or_default().to_string()),
        );
        pages += 1;
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![on_workgroup, on_other_database, on_cluster],
        "paging must return every session exactly once, in order"
    );
    assert!(pages > 1, "three sessions must not fit in one page of 1");
}

/// `ListSessions` refuses the filter combinations the operation documents as
/// mutually exclusive, and a `Status` the model does not declare.
#[tokio::test]
async fn list_sessions_refuses_the_filter_combinations_it_documents_as_exclusive() {
    let emulator = Emulator::new();
    let session = open_session(&emulator, &cluster(), 90).await;

    let cases: [(&str, Value); 6] = [
        // "When you provide SessionId, you can't specify Status,
        // ClusterIdentifier, WorkgroupName, or Database."
        (
            "SessionId with a status",
            json!({"SessionId": session, "Status": "AVAILABLE"}),
        ),
        (
            "SessionId with a cluster",
            json!({"SessionId": session, "ClusterIdentifier": "my-cluster"}),
        ),
        (
            "SessionId with a workgroup",
            json!({"SessionId": session, "WorkgroupName": "my-workgroup"}),
        ),
        (
            "SessionId with a database",
            json!({"SessionId": session, "Database": "dev"}),
        ),
        // "When providing ClusterIdentifier, then WorkgroupName can't be
        // specified."
        (
            "both a cluster and a workgroup",
            json!({"ClusterIdentifier": "my-cluster", "WorkgroupName": "my-workgroup"}),
        ),
        // Outside `SessionStatusString`'s declared values.
        ("a status the model does not declare", json!({"Status": "available"})),
    ];

    for (label, request) in &cases {
        let (status, response) = emulator.call("ListSessions", request).await;
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

    // A continuation token this operation never issued is refused too.
    for token in ["not-hexadecimal", "6162636465"] {
        let (status, response) = emulator
            .call("ListSessions", &json!({"NextToken": token}))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {response}"
        );
    }
}

/// A session stands in for the connection it was opened with: a statement that
/// names only its `SessionId` runs on that session's warehouse and database,
/// and reaches what an earlier statement of the session wrote. A `SessionId`
/// that names no session is a missing resource.
#[tokio::test]
async fn a_statement_naming_only_a_session_runs_on_that_session_s_connection() {
    let emulator = Emulator::new();
    let session = open_session(&emulator, &cluster(), 300).await;

    // Written through the session, with no warehouse or database named.
    for sql in [
        "CREATE TABLE in_session (id INTEGER)",
        "INSERT INTO in_session (id) VALUES (7)",
    ] {
        let submitted = emulator
            .execute_and_wait(&json!({"SessionId": session, "Sql": sql}))
            .await;
        assert_eq!(
            submitted["Status"], "FINISHED",
            "case {sql}: a statement in a live session must run, got {submitted}"
        );
        assert_eq!(
            submitted["Database"], "dev",
            "case {sql}: the statement runs on the session's database, got {submitted}"
        );
        assert_eq!(
            submitted["ClusterIdentifier"], "my-cluster",
            "case {sql}: the statement runs on the session's cluster, got {submitted}"
        );
    }

    // And read back through the connection the session was opened with, so the
    // session named the same database and not one of its own.
    let read = emulator.run("SELECT id FROM in_session").await;
    let fetched = emulator.ok("GetStatementResult", &json!({"Id": read})).await;
    assert_eq!(
        fetched["Records"],
        json!([[{"longValue": 7}]]),
        "a session's writes belong to the database it was opened with, got {fetched}"
    );

    // A SessionId no session carries names no connection at all.
    let (status, response) = emulator
        .call(
            "ExecuteStatement",
            &json!({"SessionId": "3dc6a4b5-5e2b-4b5a-9f1d-6b6d2b6c7a8e", "Sql": "SELECT 1"}),
        )
        .await;
    assert_eq!(
        status, 404,
        "a SessionId naming no session is not found, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {response}"
    );

    // And a session named BESIDE a warehouse names two targets, which is
    // refused before any session is looked up at all.
    let (status, response) = emulator
        .call(
            "ExecuteStatement",
            &json!({
                "SessionId": "3dc6a4b5-5e2b-4b5a-9f1d-6b6d2b6c7a8e",
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "Sql": "SELECT 1",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a session beside a cluster names two targets, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "expected ValidationException, got {response}"
    );

    // The session this suite opened is still the only one, so nothing above
    // created one on the way past.
    let page = emulator.ok("ListSessions", &json!({})).await;
    assert_eq!(
        list(&page, "Sessions").len(),
        1,
        "only the session that was asked for exists, got {page}"
    );
}

/// A session's `Status` is derived from the clock and from the statements that
/// ran in it: BUSY while a statement of the session has not reached its
/// terminal status, AVAILABLE once it has, and CLOSED once the session's
/// `SessionTtl` has passed — at which point it can no longer run SQL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_s_status_follows_its_statements_and_its_ttl() {
    let emulator = Emulator::new();

    // Submitted without waiting, and its turn on the database is HELD by this
    // test, so the statement is certainly still in flight when the session is
    // listed: the session reports BUSY and names the statement.
    let held = emulator.hold_turn("my-cluster", "dev");
    let mut request = cluster();
    request["Sql"] = json!(SLOW_SQL);
    request["SessionKeepAliveSeconds"] = json!(300);
    let submitted = emulator.ok("ExecuteStatement", &request).await;
    let session = submitted["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("asking to keep a session alive must mint one: {submitted}"))
        .to_string();
    let statement = id_of(&submitted);

    let busy = emulator
        .ok("ListSessions", &json!({"SessionId": session}))
        .await;
    let listed = list(&busy, "Sessions");
    assert_eq!(listed.len(), 1, "expected the one session, got {busy}");
    assert_eq!(
        listed[0]["Status"], "BUSY",
        "a session whose statement is still running is BUSY, got {busy}"
    );
    assert_eq!(
        listed[0]["CurrentStatementId"], json!(statement),
        "a BUSY session names the statement it is running, got {busy}"
    );

    // Once the statement's execution has finished, the session is ready for the
    // next one and names no statement. Releasing the turn lets it run, and the
    // record it reports is what says it is done — polled for, never waited out.
    drop(held);
    wait_for_terminal(&emulator, "the session's statement", &statement).await;
    let available = emulator
        .ok("ListSessions", &json!({"SessionId": session}))
        .await;
    let listed = list(&available, "Sessions");
    assert_eq!(
        listed[0]["Status"], "AVAILABLE",
        "a session with no running statement is AVAILABLE, got {available}"
    );
    assert!(
        listed[0].get("CurrentStatementId").is_none(),
        "an AVAILABLE session names no running statement, got {available}"
    );

    // A session kept alive for no time at all is closed by the clock: it is
    // reported only when CLOSED is asked for, and it runs nothing more.
    let mut request = cluster();
    request["Sql"] = json!("SELECT 2");
    request["SessionKeepAliveSeconds"] = json!(0);
    request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
    let expired = emulator.ok("ExecuteStatement", &request).await;
    let closed = expired["SessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("asking to keep a session alive must mint one: {expired}"))
        .to_string();

    let default_page = emulator.ok("ListSessions", &json!({})).await;
    let listed: Vec<String> = list(&default_page, "Sessions")
        .iter()
        .map(|session| session["SessionId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![session],
        "only AVAILABLE and BUSY sessions are listed by default, got {default_page}"
    );
    let closed_page = emulator
        .ok("ListSessions", &json!({"Status": "CLOSED"}))
        .await;
    let listed: Vec<String> = list(&closed_page, "Sessions")
        .iter()
        .map(|session| session["SessionId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed,
        vec![closed.clone()],
        "a closed session is reported when CLOSED is asked for, got {closed_page}"
    );
    let (status, response) = emulator
        .call(
            "ExecuteStatement",
            &json!({"SessionId": closed, "Sql": "SELECT 3"}),
        )
        .await;
    assert_eq!(
        status, 404,
        "a closed session can no longer run SQL statements, got {status} {response}"
    );
}

// ---------------------------------------------------------------------------
// DescribeTable
// ---------------------------------------------------------------------------

/// One expected column of a described table: its name, the `typeName` it must
/// report, and the `precision` and `scale` its declaration calls for.
struct ExpectedColumn {
    name: &'static str,
    type_name: &'static str,
    precision: i64,
    scale: i64,
}

/// One row of the `DescribeTable` table: the DDL that creates it, its name, and
/// the columns it must be described with.
struct TableCase {
    ddl: &'static str,
    table: &'static str,
    columns: &'static [ExpectedColumn],
}

/// `DescribeTable` reports the columns a table of the named database declares,
/// in declaration order, each one carrying the schema and table it belongs to.
#[tokio::test]
async fn describe_table_reports_the_columns_a_table_declares() {
    let cases = [
        TableCase {
            ddl: "CREATE TABLE people (id INTEGER, name VARCHAR(32), amt DECIMAL(10,2))",
            table: "people",
            columns: &[
                ExpectedColumn { name: "id", type_name: "int4", precision: 0, scale: 0 },
                ExpectedColumn { name: "name", type_name: "varchar", precision: 32, scale: 0 },
                ExpectedColumn { name: "amt", type_name: "numeric", precision: 10, scale: 2 },
            ],
        },
        TableCase {
            ddl: "CREATE TABLE one_column (only_col CHAR(4))",
            table: "one_column",
            columns: &[ExpectedColumn {
                name: "only_col",
                type_name: "bpchar",
                precision: 4,
                scale: 0,
            }],
        },
        TableCase {
            ddl: "CREATE TABLE temporal (at TIMESTAMP, on_day DATE, flag BOOLEAN)",
            table: "temporal",
            columns: &[
                ExpectedColumn { name: "at", type_name: "timestamp", precision: 0, scale: 0 },
                ExpectedColumn { name: "on_day", type_name: "date", precision: 0, scale: 0 },
                ExpectedColumn { name: "flag", type_name: "bool", precision: 0, scale: 0 },
            ],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        emulator.run(case.ddl).await;
    }

    for case in &cases {
        let (ddl, table) = (case.ddl, case.table);
        let mut request = cluster();
        request["Table"] = json!(table);
        request["Schema"] = json!("public");
        let described = emulator.ok("DescribeTable", &request).await;
        assert_eq!(
            described["TableName"], json!(table),
            "case {ddl}: expected TableName {table}, got {described}"
        );
        let reported = list(&described, "ColumnList");
        assert_eq!(
            reported.len(),
            case.columns.len(),
            "case {ddl}: one entry per declared column, got {described}"
        );
        for (index, expected) in case.columns.iter().enumerate() {
            let column = &reported[index];
            let name = expected.name;
            assert_eq!(
                column["name"], json!(name),
                "case {ddl}: column {index} must be {name}, got {column}"
            );
            assert_eq!(
                column["typeName"], json!(expected.type_name),
                "case {ddl}: column {name} must report typeName {}, got {column}",
                expected.type_name
            );
            assert_eq!(
                number(column, "precision"),
                expected.precision,
                "case {ddl}: column {name} must report precision {}, got {column}",
                expected.precision
            );
            assert_eq!(
                number(column, "scale"),
                expected.scale,
                "case {ddl}: column {name} must report scale {}, got {column}",
                expected.scale
            );
            assert_eq!(
                column["tableName"], json!(table),
                "case {ddl}: column {name} belongs to {table}, got {column}"
            );
            assert_eq!(
                column["schemaName"], json!("public"),
                "case {ddl}: column {name} is in the default schema, got {column}"
            );
        }

        // The same columns are reported when no schema is named, since "if no
        // schema is specified, then matching tables for all schemas are
        // returned".
        let mut request = cluster();
        request["Table"] = json!(table);
        let without_schema = emulator.ok("DescribeTable", &request).await;
        assert_eq!(
            without_schema["ColumnList"], described["ColumnList"],
            "case {ddl}: naming no schema must describe the same table, got {without_schema}"
        );
    }

    // A request that names no table describes every table of the database, so
    // the column list spans all of them.
    let all = emulator.ok("DescribeTable", &cluster()).await;
    let spanned = list(&all, "ColumnList");
    let expected: usize = cases.iter().map(|case| case.columns.len()).sum();
    assert_eq!(
        spanned.len(),
        expected,
        "naming no table describes every table of the database, got {all}"
    );
    assert!(
        all.get("TableName").is_none(),
        "a request naming no table reports no single table name, got {all}"
    );
}

/// `DescribeTable` pages its `ColumnList` with an opaque `NextToken`, and
/// refuses a token it never issued.
#[tokio::test]
async fn describe_table_pages_its_column_list() {
    let emulator = Emulator::new();
    emulator
        .run("CREATE TABLE paged (a INTEGER, b VARCHAR(8), c DECIMAL(6,3))")
        .await;

    let mut collected: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0_usize;
    loop {
        let mut request = cluster();
        request["Table"] = json!("paged");
        request["MaxResults"] = json!(1);
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("DescribeTable", &request).await;
        let columns = list(&page, "ColumnList");
        assert!(
            columns.len() <= 1,
            "page {pages}: MaxResults 1 holds at most one column, got {page}"
        );
        collected.extend(
            columns
                .iter()
                .map(|column| column["name"].as_str().unwrap_or_default().to_string()),
        );
        pages += 1;
        match page.get("NextToken").and_then(Value::as_str) {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        collected,
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
        "paging must return every column exactly once, in declaration order"
    );
    assert!(pages > 1, "three columns must not fit in one page of 1");

    for token in ["not-hexadecimal", "6162636465"] {
        let mut request = cluster();
        request["Table"] = json!("paged");
        request["NextToken"] = json!(token);
        let (status, response) = emulator.call("DescribeTable", &request).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {response}"
        );
    }
}

/// A table the named database's catalog does not hold is not found, and a
/// request that names no data warehouse is refused.
#[tokio::test]
async fn describe_table_refuses_what_the_catalog_does_not_hold() {
    let emulator = Emulator::new();
    emulator.run("CREATE TABLE present (id INTEGER)").await;

    let not_found: [(&str, Value); 4] = [
        ("a table no database holds", json!({"Table": "absent"})),
        (
            "a table of another database",
            json!({"Database": "reporting", "Table": "present"}),
        ),
        (
            "a database no statement has opened",
            json!({"Database": "never-used", "Table": "present"}),
        ),
        (
            "a schema this database does not hold",
            json!({"Schema": "other", "Table": "present"}),
        ),
    ];
    for (label, overrides) in &not_found {
        let mut request = cluster();
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            request[member] = value.clone();
        }
        let (status, response) = emulator.call("DescribeTable", &request).await;
        assert_eq!(
            status, 404,
            "case {label}: expected 404, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {label}: expected ResourceNotFoundException, got {response}"
        );
    }

    let refused: [(&str, Value); 3] = [
        ("neither a cluster nor a workgroup", json!({"Database": "dev"})),
        (
            "both a cluster and a workgroup",
            json!({
                "ClusterIdentifier": "my-cluster",
                "WorkgroupName": "my-workgroup",
                "Database": "dev",
            }),
        ),
        (
            "two authorization methods at once",
            json!({
                "ClusterIdentifier": "my-cluster",
                "Database": "dev",
                "DbUser": "awsuser",
                "SecretArn": "arn:aws:secretsmanager:us-east-1:123456789012:secret:db-AbCdEf",
            }),
        ),
    ];
    for (label, request) in &refused {
        let (status, response) = emulator.call("DescribeTable", request).await;
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
}
