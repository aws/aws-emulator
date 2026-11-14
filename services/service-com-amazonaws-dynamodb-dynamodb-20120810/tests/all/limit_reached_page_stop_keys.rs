// SPDX-License-Identifier: CC0-1.0

//! What a paginated read reports when it stopped because it reached the caller's `Limit`,
//! and how the scan surface words a clause about a member outside its declared range.
//!
//! The rules pinned here:
//!
//! 1. a paginated read reports the key it STOPPED AT whenever it stopped short of the end
//!    of what it walked — rows left to evaluate, OR the caller's `Limit` reached, whether
//!    or not a row is left. "If LastEvaluatedKey is not empty, it does not necessarily
//!    mean that there is more data in the result set. The only way to know when you have
//!    reached the end of the result set is when LastEvaluatedKey is empty"
//!    (`Scan$LastEvaluatedKey`), so a walk whose last row filled a page asks for one page
//!    more, and the page that reports nothing is the only one that ends the walk. The rule
//!    is the paginated READ's, not one operation's: `Scan`, `Query` and the `SELECT` of
//!    `ExecuteStatement` (whose cursor is a `NextToken` beside the same key) all follow
//!    it;
//! 2. a numeric member of a `Scan` outside the `@range` its model declares is refused by a
//!    clause in the ITEM-REQUEST form — the member named by the spelling the model gives
//!    it on the wire, and no value echoed — the form its sibling reads state, while a
//!    control-plane listing's page size keeps the request validator's form, which names
//!    the lowerCamel input path and echoes the number.
//!
//! The rows differ along what each rule generalises over: for rule 1 the page size (one
//! row per page, several, exactly the row count, one more than it), whether the limit
//! divides the row count, the operation, and the cursor member the operation pages by;
//! for rule 2 the member (`Limit`, `Segment`, `TotalSegments`), which end of the range was
//! broken, and how far past it the value went.
//!
//! Every bound named here comes from the Smithy model:
//! `smithy_query(query="operation", operation="Scan")` gives `Scan$Limit`
//! `@range{min:1}`, `Scan$TotalSegments` `@range{min:1,max:1000000}` and `Scan$Segment`
//! `@range{min:0,max:999999}`, and `ListBackups$Limit` `@range{min:1,max:100}`.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The one partition every row below sits in, so a `Query` walks exactly what a `Scan`
/// walks and the two readers are compared over the same rows.
const PARTITION: &str = "p";

/// How many rows the paged table holds. Small enough that every page size from one row to
/// one more than the whole table is driven to the end of the walk.
const ROWS: usize = 5;

/// The table the paging rows are read from: one partition key and one numeric sort key, so
/// the walk has a total order a resumed page carries on from.
async fn paged_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
            ],
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
    for row in 0..ROWS {
        ok(
            router,
            "PutItem",
            json!({"TableName": name,
                   "Item": {"pk": {"S": PARTITION}, "sk": {"N": row.to_string()}}}),
        )
        .await;
    }
}

/// One page of `operation` (`Scan` or `Query`) over the whole partition, resumed from
/// `start` where a previous page reported one.
fn page_request(operation: &str, table: &str, limit: usize, start: &Value) -> Value {
    let mut request = if operation == "Query" {
        json!({"TableName": table, "Limit": limit,
               "KeyConditionExpression": "pk = :p",
               "ExpressionAttributeValues": {":p": {"S": PARTITION}}})
    } else {
        json!({"TableName": table, "Limit": limit})
    };
    if !start.is_null() {
        request["ExclusiveStartKey"] = start.clone();
    }
    request
}

/// The `sk` of every item a page reported, in the order it reported them.
fn sort_keys(page: &Value) -> Vec<String> {
    page["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["sk"]["N"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// How many pages a walk of `rows` rows at `limit` rows a page takes: one page per full
/// page of rows, and — when the limit was reached by the last row there was — one more,
/// because that page reported where it stopped.
fn expected_pages(rows: usize, limit: usize) -> usize {
    match rows {
        0 => 1,
        rows if rows % limit == 0 => rows / limit + 1,
        rows => rows.div_ceil(limit),
    }
}

#[tokio::test]
async fn a_page_that_reached_its_limit_reports_where_it_stopped() {
    let router = emulator();
    paged_table(&router, "PagedByLimit").await;

    // Every page size from one row per page to one more than the table holds, driven
    // through both paginated item readers. The rows where the limit DIVIDES the row count
    // (1, 5) are the ones that end on a full page, which is what makes the walk ask for
    // one page more.
    for operation in ["Scan", "Query"] {
        for limit in 1..=ROWS + 1 {
            let case = format!("{operation} at {limit} row(s) a page");
            let mut collected: Vec<String> = Vec::new();
            let mut start = Value::Null;
            let mut pages = 0usize;
            loop {
                let page = ok(
                    &router,
                    operation,
                    page_request(operation, "PagedByLimit", limit, &start),
                )
                .await;
                pages += 1;
                assert!(
                    pages <= ROWS + 2,
                    "case {case}: the walk must end, got page {pages} of {page}"
                );
                let reported = sort_keys(&page);
                assert!(
                    reported.len() <= limit,
                    "case {case}: a page reports at most the Limit, got {} in {page}",
                    reported.len()
                );
                collected.extend(reported.iter().cloned());
                start = page["LastEvaluatedKey"].clone();
                // A page that filled its Limit reports where it stopped whether or not a
                // row is left, so the only page that ends the walk is one that stopped
                // for want of rows.
                assert_eq!(
                    !start.is_null(),
                    reported.len() == limit,
                    "case {case}: a page reports a stop key exactly when it reached its \
                     Limit, got {page}"
                );
                if start.is_null() {
                    break;
                }
            }
            assert_eq!(
                collected,
                (0..ROWS).map(|row| row.to_string()).collect::<Vec<String>>(),
                "case {case}: the pages hold every row once, in key order"
            );
            let expected = expected_pages(ROWS, limit);
            assert_eq!(
                pages, expected,
                "case {case}: {ROWS} row(s) take {expected} page(s), got {pages}"
            );
        }
    }
}

#[tokio::test]
async fn a_statement_page_that_reached_its_limit_reports_its_cursor() {
    let router = emulator();
    paged_table(&router, "PagedStatement").await;
    let statement = format!("SELECT * FROM \"PagedStatement\" WHERE pk = '{PARTITION}'");

    // The same rule through the PartiQL reader, whose cursor is a `NextToken` beside the
    // key the page stopped on: the rows differ in the page size and in whether it divides
    // the row count, and the walk is driven by the token the previous page reported.
    for limit in 1..=ROWS + 1 {
        let case = format!("a statement at {limit} row(s) a page");
        let mut collected: Vec<String> = Vec::new();
        let mut token = Value::Null;
        let mut pages = 0usize;
        loop {
            let mut request = json!({"Statement": &statement, "Limit": limit});
            if !token.is_null() {
                request["NextToken"] = token.clone();
            }
            let page = ok(&router, "ExecuteStatement", request).await;
            pages += 1;
            assert!(
                pages <= ROWS + 2,
                "case {case}: the walk must end, got page {pages} of {page}"
            );
            let reported = sort_keys(&page);
            collected.extend(reported.iter().cloned());
            token = page["NextToken"].clone();
            assert_eq!(
                !token.is_null(),
                reported.len() == limit,
                "case {case}: a page reports a cursor exactly when it reached its Limit, \
                 got {page}"
            );
            // The key the statement stopped on travels with that cursor, so a client
            // reading either member sees the same stop.
            assert_eq!(
                !page["LastEvaluatedKey"].is_null(),
                !token.is_null(),
                "case {case}: the stop key and the cursor are reported together, got {page}"
            );
            if token.is_null() {
                break;
            }
        }
        assert_eq!(
            collected,
            (0..ROWS).map(|row| row.to_string()).collect::<Vec<String>>(),
            "case {case}: the pages hold every row once"
        );
        let expected = expected_pages(ROWS, limit);
        assert_eq!(
            pages, expected,
            "case {case}: {ROWS} row(s) take {expected} page(s), got {pages}"
        );
    }
}

#[tokio::test]
async fn a_scan_states_a_range_clause_in_the_form_that_member_takes() {
    let router = emulator();
    paged_table(&router, "ScanClauses").await;

    // Rows differ in the member, which end of its published range the value broke, and how
    // far past the bound it went. The page size takes the request validator's form, which
    // echoes the rejected value, where the parallel-scan bounds beside it take the
    // item-request form; the last row is the CONTRAST that the form is not the operation's
    // alone: a control-plane listing's page size, one below the same minimum, echoes it too.
    let cases: [(&str, &str, Value, &str); 6] = [
        (
            "a page size one below its range minimum",
            "Scan",
            json!({"TableName": "ScanClauses", "Limit": 0}),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1",
        ),
        (
            "a page size far below that minimum states the same requirement",
            "Scan",
            json!({"TableName": "ScanClauses", "Limit": -7}),
            "1 validation error detected: Value '-7' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1",
        ),
        (
            "a segment count one below its minimum",
            "Scan",
            json!({"TableName": "ScanClauses", "Segment": 0, "TotalSegments": 0}),
            "1 validation error detected: Value at 'TotalSegments' failed to satisfy \
             constraint: Member must have value greater than or equal to 1",
        ),
        (
            "a segment count one past its maximum",
            "Scan",
            json!({"TableName": "ScanClauses", "Segment": 0, "TotalSegments": 1_000_001}),
            "1 validation error detected: Value at 'TotalSegments' failed to satisfy \
             constraint: Member must have value less than or equal to 1000000",
        ),
        (
            "a segment one below its zero minimum",
            "Scan",
            json!({"TableName": "ScanClauses", "Segment": -1, "TotalSegments": 2}),
            "1 validation error detected: Value at 'Segment' failed to satisfy constraint: \
             Member must have value greater than or equal to 0",
        ),
        (
            "a control-plane page size one below the same minimum",
            "ListBackups",
            json!({"Limit": 0}),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy \
             constraint: Member must have value greater than or equal to 1",
        ),
    ];

    for (case, operation, request, expected) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected a 400 ValidationException, got {status} \
             with {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // Each member AT its published bound is served rather than refused, so the clauses
    // above state the bound and not an off-by-one.
    for (case, request) in [
        (
            "the page size at its minimum",
            json!({"TableName": "ScanClauses", "Limit": 1}),
        ),
        (
            "the segment count at its minimum",
            json!({"TableName": "ScanClauses", "Segment": 0, "TotalSegments": 1}),
        ),
        (
            "the segment at its maximum",
            json!({"TableName": "ScanClauses", "Segment": 999_999, "TotalSegments": 1_000_000}),
        ),
    ] {
        let (status, body) = call(&router, "Scan", request).await;
        let stated = message(&body);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {case:?}: a value at its published bound is no constraint violation, got \
             {status} with {stated:?}"
        );
    }
}
