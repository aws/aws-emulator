// SPDX-License-Identifier: CC0-1.0

//! How the PartiQL surface refuses text that states none of the verbs it serves, and how
//! deeply a statement it does serve may nest.
//!
//! THE RULE: this surface runs "reads and singleton writes on data stored in DynamoDB,
//! using PartiQL" — the `SELECT`, `INSERT`, `UPDATE` and `DELETE` statements — so text that
//! opens with none of those verbs manipulates no data, and the refusal names the position
//! the parse of it stopped at: text that reads as ONE complete expression (the language's
//! other statement form, a query) states no data manipulation, while text with a token left
//! over past that expression stopped at that token, which nothing in the dialect can follow
//! an expression with.
//!
//! The rows differ along WHAT the text spells instead: a bare name, a quoted name, a
//! literal, a predicate, a misspelling of each of the four verbs, a statement of another
//! dialect, and words that are no statement in any dialect. The control that follows shows
//! the refusal is about the VERB and not a blanket answer: each of the four verbs, spelled
//! correctly, is served against the same table.
//!
//! Text whose leading keywords name one of the statement language's OWN other operations —
//! the schema statements — is a different fact and earns a different refusal, which
//! `tests/all/partiql_unsupported_operations.rs` pins.
//!
//! The second test pins the other half of what a statement's SHAPE may be: how deeply it
//! nests is the client's choice, so a statement past this surface's ceiling is refused as a
//! bad request and the surface keeps serving.
//!
//! Every row is driven over the wire through `ExecuteStatement`, and the control reads the
//! store back through `GetItem`, so nothing is asserted that a client could not observe.

use serde_json::json;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row below names.
const TABLE: &str = "NonDmlStatements";

/// The fixed sentence a parse defect is stated with, over the parser's phrase for it.
fn parse_failure(phrase: &str) -> String {
    format!("Statement wasn't well formed, can't be processed: {phrase}")
}

/// The parser's phrase for text that read as one complete expression and manipulates no
/// data.
const NO_DATA_MANIPULATION: &str = "Expected data manipulation";

/// The parser's phrase for text a token is left over past the expression it read in.
const TRAILING_TOKEN: &str = "Unexpected token after expression";

/// An emulator holding one active table with one item in it, so a statement that IS served
/// can be seen to have run.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the non-DML statement table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "a"}, "n": {"N": "1"}}}),
    )
    .await;
    router
}

#[tokio::test]
async fn text_that_states_none_of_the_served_verbs_manipulates_no_data() {
    let router = seeded().await;

    let rows: Vec<(&str, String, &str)> = vec![
        ("a bare name", TABLE.to_string(), NO_DATA_MANIPULATION),
        (
            "a quoted name",
            format!("\"{TABLE}\""),
            NO_DATA_MANIPULATION,
        ),
        (
            "a string literal",
            "'not a statement'".to_string(),
            NO_DATA_MANIPULATION,
        ),
        ("a predicate", "pk = 'a'".to_string(), NO_DATA_MANIPULATION),
        (
            "a name and a word after it",
            format!("{TABLE} rows"),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled SELECT",
            format!("SELCT * FROM \"{TABLE}\""),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled INSERT",
            format!("INSRT INTO \"{TABLE}\" VALUE {{'pk':'z'}}"),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled UPDATE",
            format!("UPDTE \"{TABLE}\" SET n = 2 WHERE pk = 'a'"),
            TRAILING_TOKEN,
        ),
        (
            "a misspelled DELETE",
            format!("DELET FROM \"{TABLE}\" WHERE pk = 'a'"),
            TRAILING_TOKEN,
        ),
        (
            "a statement of another dialect",
            format!("TRUNCATE \"{TABLE}\""),
            TRAILING_TOKEN,
        ),
        (
            "a statement that reads a schema",
            format!("DESCRIBE \"{TABLE}\""),
            TRAILING_TOKEN,
        ),
        (
            "text that is no statement in any dialect",
            "this is not a statement".to_string(),
            TRAILING_TOKEN,
        ),
    ];

    for (case, statement, reason) in &rows {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message(&answered).as_str()
            ),
            (400, "ValidationException", parse_failure(reason).as_str()),
            "case {case}: {statement:?} manipulates no data, got {status} with \
             {answered}"
        );
    }

    // The control: each verb this surface DOES serve runs against the same table, so the
    // refusals above turn on the verb the text opens with.
    for (case, statement) in [
        ("SELECT", format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'")),
        (
            "UPDATE",
            format!("UPDATE \"{TABLE}\" SET n = 2 WHERE pk = 'a'"),
        ),
        (
            "INSERT",
            format!("INSERT INTO \"{TABLE}\" VALUE {{'pk':'z'}}"),
        ),
        ("DELETE", format!("DELETE FROM \"{TABLE}\" WHERE pk = 'z'")),
    ] {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": &statement}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: {statement:?} states a manipulation of data and must run, got \
             {status} with {answered}"
        );
    }

    // What those statements did is what the store now holds: the UPDATE landed on the
    // seeded item and the INSERT's row was inserted and then deleted again.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "a"}, "n": {"N": "2"}}),
        "case the served UPDATE: the item must carry what the statement set, got {read}"
    );
    let gone = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "z"}}}),
    )
    .await;
    assert!(
        gone.get("Item").is_none(),
        "case the served DELETE: the inserted item must be gone, got {gone}"
    );
}

/// RULE: how deeply a statement NESTS is the client's choice, so every walk over it — the
/// parse and the render that follows — is held to this surface's own ceiling: a statement
/// past it is refused as a bad request, and the emulator keeps serving.
///
/// The rows differ along WHAT nests (a parenthesised predicate, a chain of `NOT`s, nested
/// `list_append` calls) and along the depth (inside the ceiling, and far past it), and each
/// row is followed by a served statement, so a refusal that took the process with it would
/// show as the next row failing rather than as this one passing.
#[tokio::test]
async fn a_statement_nested_past_the_ceiling_is_refused_and_the_surface_keeps_serving() {
    let router = seeded().await;

    // The ceiling the surface holds a statement's grammar to, which this suite reads from
    // the crate rather than restating: a depth inside it is served, one past it refused.
    let ceiling = service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_STATEMENT_DEPTH;

    for (case, statement, admitted) in [
        (
            "a predicate inside the ceiling",
            format!(
                "SELECT * FROM \"{TABLE}\" WHERE {}pk = 'a'{}",
                "(".repeat(ceiling - 1),
                ")".repeat(ceiling - 1)
            ),
            true,
        ),
        (
            "a predicate past the ceiling",
            format!(
                "SELECT * FROM \"{TABLE}\" WHERE {}pk = 'a'{}",
                "(".repeat(ceiling + 8),
                ")".repeat(ceiling + 8)
            ),
            false,
        ),
        (
            "a chain of NOTs far past the ceiling",
            format!(
                "SELECT * FROM \"{TABLE}\" WHERE {}(pk = 'a'){}",
                "NOT (".repeat(ceiling * 8),
                ")".repeat(ceiling * 8)
            ),
            false,
        ),
        (
            "nested list_append calls far past the ceiling",
            format!(
                "UPDATE \"{TABLE}\" SET l = {}l, ['x']{} WHERE pk = 'a'",
                "list_append(".repeat(ceiling * 8),
                ")".repeat(ceiling * 8)
            ),
            false,
        ),
    ] {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": &statement}),
        )
        .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case}: a statement inside the ceiling must be served, got {status} \
                 with {answered}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ValidationException"),
                "case {case}: a statement past the ceiling must be refused as a bad request, \
                 got {status} with {answered}"
            );
        }

        // The surface is still there: the refusal cost that caller its answer and nothing
        // else.
        let (status, served) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'")}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the next statement must still be served, got {status} with {served}"
        );
    }
}
