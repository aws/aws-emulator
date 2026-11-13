// SPDX-License-Identifier: CC0-1.0

//! Reading the RESULT of a statement, driven over the wire through the shared
//! harness in [`super::emulator`]: every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>`, so what is asserted is the
//! emulator's HTTP behaviour and not a handler's return value.
//!
//! Two rules are under test.
//!
//! 1. A statement that produced NO result is refused with the missing-result
//!    error at the status the service answers that refusal with — the
//!    client-input status, because the statement exists and its outcome is
//!    readable and what is wrong is the request asking for a result of a
//!    statement that produced none. Asserted over a table whose rows differ in
//!    the terminal status the statement reached, in why it has no result, and
//!    in which reader the result is read through.
//!
//! 2. The service's documented 100 MB ceiling is on what ONE result read
//!    answers with, not on the result set a statement produces: a statement
//!    whose result set is larger runs to completion and is served across pages
//!    linked by `NextToken`, with `TotalNumRows` reporting the whole count and
//!    every row coming back exactly once. Asserted over a table of result-set
//!    sizes around the page boundary, and then over a result set of tens of
//!    megabytes, which is far more than one page carries.

use serde_json::{json, Value};

use super::emulator::{
    error_code, id_of, list, number, wait_for_terminal, Emulator, SLOW_SQL, WAIT_SECONDS,
};

use service_com_amazonaws_redshiftdata_redshiftdata::state::RESULT_PAGE_ROWS;

/// The fixed sentence the Data API refuses a result read for a statement that
/// produced no result with.
const NO_RESULT_MESSAGE: &str =
    "Query does not have result. Please check query status with DescribeStatement.";

/// The connection every submission below is made with: one documented
/// authorization combination (a provisioned cluster as a database user).
fn connection() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// One row of the no-result table: how the statement ends up without a result,
/// and which reader asks for it.
struct NoResultCase {
    label: &'static str,
    /// The statement to submit.
    sql: &'static str,
    /// The `ResultFormat` to submit it with, which decides the reader that
    /// serves its result.
    format: Option<&'static str>,
    /// The reader to ask.
    reader: &'static str,
    /// Whether the statement has to be CANCELLED to reach its terminal status.
    cancelled: bool,
    /// The terminal status it must have reached when the result is read.
    status: &'static str,
}

/// A statement that produced no result is refused with the missing-result error
/// at the status the service answers that refusal with, in its fixed words,
/// whatever terminal status the statement reached and whichever reader asks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_with_no_result_is_refused_as_a_client_input_failure() {
    let cases = [
        NoResultCase {
            label: "a FAILED statement, read as JSON",
            sql: "SELECT id FROM no_such_table",
            format: None,
            reader: "GetStatementResult",
            cancelled: false,
            status: "FAILED",
        },
        NoResultCase {
            label: "a FAILED statement, read as CSV",
            sql: "SELECT id FROM no_such_table",
            format: Some("CSV"),
            reader: "GetStatementResultV2",
            cancelled: false,
            status: "FAILED",
        },
        NoResultCase {
            label: "a FINISHED DDL statement, read as JSON",
            sql: "CREATE TABLE no_result_ddl (id INTEGER)",
            format: None,
            reader: "GetStatementResult",
            cancelled: false,
            status: "FINISHED",
        },
        NoResultCase {
            label: "a FINISHED INSERT, read as CSV",
            sql: "INSERT INTO pages (id) VALUES (1)",
            format: Some("CSV"),
            reader: "GetStatementResultV2",
            cancelled: false,
            status: "FINISHED",
        },
        NoResultCase {
            label: "an ABORTED statement, read as JSON",
            sql: SLOW_SQL,
            format: None,
            reader: "GetStatementResult",
            cancelled: true,
            status: "ABORTED",
        },
    ];

    let emulator = Emulator::new();
    emulator.run("CREATE TABLE pages (id INTEGER)").await;

    for case in &cases {
        let mut request = connection();
        request["Sql"] = json!(case.sql);
        if let Some(format) = case.format {
            request["ResultFormat"] = json!(format);
        }
        let id = if case.cancelled {
            let submitted = emulator.ok("ExecuteStatement", &request).await;
            let id = id_of(&submitted);
            let cancelled = emulator.ok("CancelStatement", &json!({"Id": id})).await;
            assert_eq!(
                cancelled["Status"],
                json!(true),
                "case {}: the statement must still have been running: {cancelled}",
                case.label
            );
            id
        } else {
            request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
            id_of(&emulator.ok("ExecuteStatement", &request).await)
        };

        let described = wait_for_terminal(&emulator, case.label, &id).await;
        assert_eq!(
            described["Status"],
            json!(case.status),
            "case {}: expected the statement to reach {}, got {described}",
            case.label,
            case.status
        );

        let (status, refused) = emulator.call(case.reader, &json!({"Id": id})).await;
        assert_eq!(
            status, 400,
            "case {}: a result read for a statement with no result is a client-input failure, \
             expected 400, got {status} {refused}",
            case.label
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {}: expected ResourceNotFoundException, got {refused}",
            case.label
        );
        assert_eq!(
            refused["Message"],
            json!(NO_RESULT_MESSAGE),
            "case {}: expected the service's own sentence, got {refused}",
            case.label
        );
    }
}

/// Build a table of `rows` rows of `(id, payload)`, the payload padded to about
/// `payload_bytes` characters, and submit the query that reads it back in id
/// order. Answers the `Id` of that query.
async fn built_and_queried(
    emulator: &Emulator,
    case: &str,
    table: &str,
    rows: usize,
    payload_bytes: usize,
) -> String {
    // The rows are built by the ENGINE, from a recursive term, so the table
    // really holds what the query below reads: `pad` grows one chunk at a time
    // until it is at least the width asked for.
    let build = format!(
        "CREATE TABLE {table} AS WITH RECURSIVE pad(p) AS (SELECT 'redshift-' UNION ALL \
         SELECT p || 'redshift-' FROM pad WHERE length(p) < {payload_bytes}), \
         ids(id) AS (SELECT 1 UNION ALL SELECT id + 1 FROM ids WHERE id < {rows}) \
         SELECT id AS id, (SELECT p FROM pad WHERE length(p) >= {payload_bytes} LIMIT 1) \
         AS payload FROM ids"
    );
    let mut request = connection();
    request["Sql"] = json!(build);
    request["WaitTimeSeconds"] = json!(WAIT_SECONDS);
    let built = emulator.ok("ExecuteStatement", &request).await;
    let described = wait_for_terminal(emulator, case, &id_of(&built)).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "case {case}: building the {rows}-row table must FINISH, got {described}"
    );

    let mut query = connection();
    query["Sql"] = json!(format!("SELECT id, payload FROM {table} ORDER BY id"));
    let submitted = emulator.ok("ExecuteStatement", &query).await;
    id_of(&submitted)
}

/// Page through a statement's result with `NextToken` until the service issues
/// none, and answer the ids it served, how many pages it took, and the
/// `TotalNumRows` every page reported.
async fn paged_ids(emulator: &Emulator, case: &str, id: &str) -> (Vec<i64>, usize, i64) {
    let mut ids: Vec<i64> = Vec::new();
    let mut pages = 0_usize;
    let mut total: i64 = -1;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"Id": id});
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let page = emulator.ok("GetStatementResult", &request).await;
        pages += 1;
        let reported = number(&page, "TotalNumRows");
        if total >= 0 {
            assert_eq!(
                reported, total,
                "case {case}: every page must report the same TotalNumRows, got {reported} \
                 after {total}"
            );
        }
        total = reported;
        let records = list(&page, "Records");
        assert!(
            !records.is_empty(),
            "case {case}: page {pages} must carry rows, got {page}"
        );
        assert!(
            records.len() <= RESULT_PAGE_ROWS,
            "case {case}: page {pages} carries {} rows, more than the {RESULT_PAGE_ROWS} a page \
             may carry",
            records.len()
        );
        for record in &records {
            let cells = record
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: a record is a list of fields: {record}"));
            ids.push(
                cells
                    .first()
                    .and_then(|cell| cell["longValue"].as_i64())
                    .unwrap_or_else(|| panic!("case {case}: a row's id is a number: {record}")),
            );
        }
        match page["NextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return (ids, pages, total),
        }
    }
}

/// Every id from 1 to `rows` comes back exactly once, in as many pages as the
/// page size calls for.
fn assert_served(case: &str, rows: usize, served: (Vec<i64>, usize, i64)) {
    let (ids, pages, total) = served;
    assert_eq!(
        total,
        i64::try_from(rows).unwrap_or(i64::MAX),
        "case {case}: TotalNumRows must report the whole result set of {rows} rows, got {total}"
    );
    assert_eq!(
        ids.len(),
        rows,
        "case {case}: the pages must carry every one of the {rows} rows, got {}",
        ids.len()
    );
    let expected_pages = rows.div_ceil(RESULT_PAGE_ROWS);
    assert_eq!(
        pages, expected_pages,
        "case {case}: {rows} rows at {RESULT_PAGE_ROWS} rows a page is {expected_pages} pages, \
         got {pages}"
    );
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        ids.len(),
        "case {case}: every id must come back exactly once, {} of {} are distinct",
        sorted.len(),
        ids.len()
    );
    assert_eq!(
        (sorted.first().copied(), sorted.last().copied()),
        (
            Some(1),
            Some(i64::try_from(rows).unwrap_or(i64::MAX))
        ),
        "case {case}: the ids served must be 1..{rows}, got {:?}..{:?}",
        sorted.first(),
        sorted.last()
    );
}

/// A result set of any size runs to completion and is served across `NextToken`
/// pages. The rows differ in how the result set sits against the page size: one
/// row, exactly one page, one row past a page, and several pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_set_is_served_across_next_token_pages() {
    let emulator = Emulator::new();
    for (index, rows) in [
        1_usize,
        RESULT_PAGE_ROWS,
        RESULT_PAGE_ROWS + 1,
        RESULT_PAGE_ROWS * 2 + 500,
    ]
    .into_iter()
    .enumerate()
    {
        let case = format!("{rows} rows");
        let table = format!("paged_{index}");
        let id = built_and_queried(&emulator, &case, &table, rows, 32).await;
        let described = wait_for_terminal(&emulator, &case, &id).await;
        assert_eq!(
            described["Status"], "FINISHED",
            "case {case}: the query must FINISH, got {described}"
        );
        assert_eq!(
            number(&described, "ResultRows"),
            i64::try_from(rows).unwrap_or(i64::MAX),
            "case {case}: the statement must report the rows it returned, got {described}"
        );
        assert_served(&case, rows, paged_ids(&emulator, &case, &id).await);
    }
}

/// Rows of the large result set below: more than the rows one page carries by a
/// wide margin, so the statement's whole result set is many pages.
const LARGE_ROWS: usize = 40_000;

/// Characters of payload each of those rows carries, so the result set is tens
/// of megabytes of result data — far more than the ceiling the service
/// documents for ONE result read, which is what makes this the case a bound on
/// the whole result set would wrongly refuse.
const LARGE_PAYLOAD_BYTES: usize = 1_000;

/// A statement whose RESULT SET is far larger than one result read may answer
/// with still runs to completion, and every row of it is served exactly once
/// across the `NextToken` pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_set_larger_than_one_read_may_answer_with_still_finishes() {
    let emulator = Emulator::new();
    let case = format!("{LARGE_ROWS} rows of {LARGE_PAYLOAD_BYTES} characters");
    let id = built_and_queried(
        &emulator,
        &case,
        "large_result",
        LARGE_ROWS,
        LARGE_PAYLOAD_BYTES,
    )
    .await;
    let described = wait_for_terminal(&emulator, &case, &id).await;
    assert_eq!(
        described["Status"], "FINISHED",
        "case {case}: a result set the service serves must not be refused, got {described}"
    );
    assert_eq!(
        number(&described, "ResultRows"),
        i64::try_from(LARGE_ROWS).unwrap_or(i64::MAX),
        "case {case}: the statement must report every row it returned, got {described}"
    );
    // `ResultSize` is the result's own size in bytes, so it is at least the
    // payload the rows carry.
    let smallest = i64::try_from(LARGE_ROWS * LARGE_PAYLOAD_BYTES).unwrap_or(i64::MAX);
    assert!(
        number(&described, "ResultSize") >= smallest,
        "case {case}: ResultSize must be at least the {smallest} bytes of payload the rows \
         carry, got {described}"
    );
    assert_served(&case, LARGE_ROWS, paged_ids(&emulator, &case, &id).await);
}
