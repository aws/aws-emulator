// SPDX-License-Identifier: CC0-1.0

//! Where the parse of text that states NO data-manipulation verb stopped, as the refusal's
//! own detail reports it.
//!
//! RULE: text this surface is handed that opens with none of the four verbs it serves is
//! read as the statement language's other form — one QUERY, which is one expression — and
//! the refusal is the fixed sentence `Statement wasn't well formed, can't be processed: `
//! over the parser's phrase for the position the parse stopped at: text that READ as one
//! complete expression with a token left over stopped at that token (`Unexpected token after
//! expression`), while text that IS one complete expression parsed as a query and
//! manipulates no data whatever it spells (`Expected data manipulation`). A misspelled verb
//! is the first of those: the misspelling is a name, which is a complete expression, and the
//! clause word after it belongs to no expression.
//!
//! The rows differ along what the rule generalises over: the FORM of the leading expression
//! (a bare name, a quoted name, a string literal, a number, a path with steps, a comparison,
//! a conjunction) and what FOLLOWS it (nothing, a clause word, a name, an operator, a
//! literal) — and, in the second test, which surface carried the statement, since the defect
//! belongs to the statement and not to the request that carried it: `ExecuteStatement`
//! refuses the call, `BatchExecuteStatement` reports it in the slot that sent it, and
//! `ExecuteTransaction` refuses the whole call behind the slot prefix.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row below names.
const TABLE: &str = "QueryFormDefects";

/// The parser's phrase for text that read as one complete expression and so manipulates no
/// data, and for text a token is left over past the expression it read in.
const NO_MANIPULATION: &str = "Expected data manipulation";
const TRAILING_TOKEN: &str = "Unexpected token after expression";

/// The whole sentence a parse defect is stated with.
fn parse_failure(phrase: &str) -> String {
    format!("Statement wasn't well formed, can't be processed: {phrase}")
}

/// An emulator holding one active table with one item in it, so a refusal can be seen to
/// have left the store alone.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "a"}, "n": {"N": "1"}}}),
    )
    .await;
    router
}

/// One row: what the text reads as, and the phrase the position its parse stopped at earns.
fn rows() -> Vec<(&'static str, String, &'static str)> {
    vec![
        (
            "a bare name, and nothing after it",
            TABLE.to_string(),
            NO_MANIPULATION,
        ),
        (
            "a quoted name, and nothing after it",
            format!("\"{TABLE}\""),
            NO_MANIPULATION,
        ),
        (
            "a string literal",
            "'not a statement'".to_string(),
            NO_MANIPULATION,
        ),
        ("a number literal", "42".to_string(), NO_MANIPULATION),
        (
            "a path with steps into an attribute",
            "detail.inner[1]".to_string(),
            NO_MANIPULATION,
        ),
        (
            "a comparison, which is one expression",
            "pk = 'a'".to_string(),
            NO_MANIPULATION,
        ),
        (
            "two comparisons joined into one expression",
            "pk = 'a' AND n = 1".to_string(),
            NO_MANIPULATION,
        ),
        (
            "a membership, which is one expression",
            "pk IN ['a','b']".to_string(),
            NO_MANIPULATION,
        ),
        (
            // The recorded case: `INSRT` is a name, so the `INTO` after it is where the
            // parse stopped.
            "a misspelled INSERT, the clause word left over",
            format!("INSRT INTO \"{TABLE}\" VALUE {{'pk':'z','n':2}}"),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled SELECT, an operator left over",
            format!("SELCT * FROM \"{TABLE}\""),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled UPDATE, a quoted name left over",
            format!("UPDTE \"{TABLE}\" SET n = 2 WHERE pk = 'a'"),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled DELETE, a clause word left over",
            format!("DELET FROM \"{TABLE}\" WHERE pk = 'a'"),
            TRAILING_TOKEN,
        ),
        (
            "a name with a name after it",
            format!("{TABLE} rows"),
            TRAILING_TOKEN,
        ),
        (
            "a comparison with a name after it",
            "pk = 'a' junk".to_string(),
            TRAILING_TOKEN,
        ),
        (
            "a name with a literal after it",
            format!("{TABLE} 'a'"),
            TRAILING_TOKEN,
        ),
        (
            "a verb of another dialect, the object left over",
            format!("TRUNCATE \"{TABLE}\""),
            TRAILING_TOKEN,
        ),
    ]
}

/// RULE: the refusal's detail names the position the parse of a query-form statement stopped
/// at, and the statement writes nothing.
#[tokio::test]
async fn a_statement_that_is_no_data_manipulation_reports_where_its_parse_stopped() {
    let router = seeded().await;

    for (case, statement, phrase) in rows() {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": &statement}),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message(&answered).as_str()
            ),
            (400, "ValidationException", parse_failure(phrase).as_str()),
            "case {case:?}: {statement:?} expected {phrase:?}, got {status} with {answered}"
        );
    }

    // Nothing any of those statements named was written: the seeded item stands as it was,
    // and the row a misspelled INSERT would have added is not there.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "a"}, "n": {"N": "1"}}),
        "case a refused statement leaves the store alone: got {read}"
    );
    let absent = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "z"}}}),
    )
    .await;
    assert!(
        absent.get("Item").is_none(),
        "case a refused statement writes nothing: got {absent}"
    );
}

/// RULE: the defect belongs to the STATEMENT, so each phrase is stated the same way
/// whichever surface carried it — per slot in a batch, and behind the slot prefix for the
/// whole call in a transaction.
#[tokio::test]
async fn every_surface_states_the_same_phrase_for_the_same_statement() {
    let router = seeded().await;
    let well_formed = format!("INSERT INTO \"{TABLE}\" VALUE {{'pk':'sibling'}}");

    for (case, statement, phrase) in rows() {
        // A batch answers 200 and carries the defect in the slot that sent it, beside a
        // sibling that still runs.
        let (status, batched) = call(
            &router,
            "BatchExecuteStatement",
            json!({"Statements": [
                {"Statement": &statement},
                {"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'")},
            ]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a batch carrying it answers 200, got {status} with {batched}"
        );
        assert_eq!(
            (
                batched["Responses"][0]["Error"]["Code"].clone(),
                batched["Responses"][0]["Error"]["Message"].clone(),
            ),
            (json!("ValidationError"), json!(parse_failure(phrase))),
            "case {case:?}: the slot states the statement's own phrase, got {batched}"
        );
        assert_eq!(
            batched["Responses"][1]["Item"]["n"],
            json!({"N": "1"}),
            "case {case:?}: the slot beside it is served, got {batched}"
        );

        // A transaction refuses the WHOLE call, behind the zero-based slot the request
        // stated the statement at — here the second of two.
        let (status, refused) = call(
            &router,
            "ExecuteTransaction",
            json!({"TransactStatements": [
                {"Statement": &well_formed},
                {"Statement": &statement},
            ]}),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                message(&refused).as_str()
            ),
            (
                400,
                "ValidationException",
                format!(
                    "Validation failed in TransactStatements[1]: {}",
                    parse_failure(phrase)
                )
                .as_str()
            ),
            "case {case:?}: the transaction states the slot and the phrase, got {status} with \
             {refused}"
        );
        // The refused transaction wrote nothing, so its well-formed sibling did not run.
        let sibling = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "sibling"}}}),
        )
        .await;
        assert!(
            sibling.get("Item").is_none(),
            "case {case:?}: a refused transaction writes none of its statements, got {sibling}"
        );
    }

    // The batches above only read, so the store still holds exactly the seeded item.
    let scanned: Value = ok(&router, "Scan", json!({"TableName": TABLE})).await;
    assert_eq!(
        scanned["Count"],
        json!(1),
        "case the whole suite: only the seeded item is stored, got {scanned}"
    );
}
