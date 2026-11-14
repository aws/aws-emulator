// SPDX-License-Identifier: CC0-1.0

//! How the PartiQL surface answers a statement that names an operation it does not run.
//!
//! THE RULE: this surface runs "reads and singleton writes on data stored in DynamoDB,
//! using PartiQL" (the `ExecuteStatement` doc) — the four data-manipulation verbs. The
//! statement language also spells SCHEMA operations, each as a verb and the kind of object
//! it acts on, so text that opens with such a pair NAMES an operation rather than failing
//! to read as a statement: it is refused 400 `ValidationException` whose message is the
//! fixed text `Unsupported operation: ` over the name of the operation the pair names, and
//! nothing is done to the table the text mentions.
//!
//! The schema operations are a catalog this suite writes out, because a statement naming
//! one of them is no statement of this surface and the model therefore declares none of
//! them: the language's `CREATE` and `DROP` of a `TABLE` and of an `INDEX`
//! (`smithy_query(query="operation", operation="ExecuteStatement")` models only the
//! statement TEXT, `Statement` of type `PartiQLStatement`, so the catalog is the language's
//! and not the model's). The rows span every entry of it — not only the `DROP TABLE` the
//! report cited — and vary what the refusal is indifferent to: the case the keywords are
//! written in, whether the object is named bare or `"quoted"`, and what follows the pair,
//! including nothing at all.
//!
//! The controls that follow pin the two edges of the rule: text that names NO operation of
//! the language earns a parse defect instead — the one for the position its parse stopped at
//! — each of the four verbs this surface serves still runs, and the table the refused
//! statements named is still ACTIVE and still holds its item. Everything is driven over the
//! wire.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every statement below names.
const TABLE: &str = "UnsupportedStatementOperations";

/// An index name a schema statement can name beside the table.
const INDEX: &str = "by-n";

/// The sentence a statement naming an operation this surface does not run earns.
fn unsupported(operation: &str) -> String {
    format!("Unsupported operation: {operation}")
}

/// An emulator holding one active table with one item in it, so what the refused
/// statements left alone can be read back.
async fn seeded() -> MultiServiceRouter {
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
    describe_when_active(&router, "the unsupported-operation table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "a"}, "n": {"N": "1"}}}),
    )
    .await;
    router
}

#[tokio::test]
async fn a_statement_naming_an_operation_this_surface_does_not_run_names_it_in_the_refusal() {
    let router = seeded().await;

    // Each row: the case, the statement, and the operation its leading keywords name.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "DROP TABLE naming a quoted table",
            format!("DROP TABLE \"{TABLE}\""),
            "DropTable",
        ),
        (
            "the same statement written in lower case",
            format!("drop table \"{TABLE}\""),
            "DropTable",
        ),
        (
            "the same statement with a bare table name and odd spacing",
            format!("DrOp   TaBlE  {TABLE}"),
            "DropTable",
        ),
        (
            "the keyword pair with nothing after it",
            "DROP TABLE".to_string(),
            "DropTable",
        ),
        (
            "CREATE TABLE naming a quoted table",
            format!("CREATE TABLE \"{TABLE}\""),
            "CreateTable",
        ),
        (
            "CREATE TABLE carrying a column list after the pair",
            format!("CREATE TABLE \"{TABLE}\" (pk VARCHAR, n INT)"),
            "CreateTable",
        ),
        (
            "CREATE INDEX naming an index of the table",
            format!("CREATE INDEX \"{INDEX}\" ON \"{TABLE}\" (n)"),
            "CreateIndex",
        ),
        (
            "DROP INDEX naming an index of the table",
            format!("DROP INDEX \"{INDEX}\" ON \"{TABLE}\""),
            "DropIndex",
        ),
        (
            "drop index written in mixed case",
            format!("Drop Index {INDEX} On {TABLE}"),
            "DropIndex",
        ),
    ];

    for (case, statement, operation) in &rows {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        let expected = unsupported(operation);
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (400, "ValidationException", expected.clone()),
            "case {case}: {statement:?} must be refused with {expected:?}, got {status} with \
             {answered}"
        );
    }

    // The same front end runs every statement surface, so a batch reports the same fact in
    // the failing statement's own slot while the statement beside it is served.
    let batched = ok(
        &router,
        "BatchExecuteStatement",
        json!({
            "Statements": [
                {"Statement": format!("DROP INDEX \"{INDEX}\" ON \"{TABLE}\"")},
                {"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'")},
            ],
        }),
    )
    .await;
    assert_eq!(
        (
            batched["Responses"][0]["Error"]["Code"].clone(),
            batched["Responses"][0]["Error"]["Message"].clone(),
        ),
        (json!("ValidationError"), json!(unsupported("DropIndex"))),
        "case a batch carrying such a statement: its slot must name the operation, got {batched}"
    );
    assert_eq!(
        batched["Responses"][1]["Item"]["n"],
        json!({"N": "1"}),
        "case a batch carrying such a statement: the slot beside it must be served, got {batched}"
    );

    // The first edge of the rule: text that names NO operation of the language is read as
    // the language's other statement form, one query, and earns the parse defect for the
    // position that parse stopped at. The rows differ in whether a token is left over past
    // the expression the text opens with (a verb of some dialect, then the object it names)
    // or the text reads as no expression at all (a keyword of the dialect, which no name may
    // be spelled as).
    for (case, statement, reason) in [
        (
            "a schema verb with no kind of object",
            format!("DROP \"{TABLE}\""),
            "Unexpected token after expression",
        ),
        (
            "a verb of another dialect",
            format!("TRUNCATE \"{TABLE}\""),
            "Unexpected token after expression",
        ),
        (
            "a kind of object with no verb",
            format!("TABLE \"{TABLE}\""),
            "Expected data manipulation",
        ),
        (
            "words that name nothing",
            "this is not a statement".to_string(),
            "Unexpected token after expression",
        ),
    ] {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": &statement}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (
                400,
                "ValidationException",
                format!("Statement wasn't well formed, can't be processed: {reason}")
            ),
            "case {case}: {statement:?} names no operation, got {status} with {answered}"
        );
    }

    // The second edge: the verbs this surface DOES serve still run, so the refusals above
    // turn on the operation the text names and are not a blanket answer.
    for (case, statement) in [
        (
            "SELECT",
            format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'"),
        ),
        (
            "INSERT",
            format!("INSERT INTO \"{TABLE}\" VALUE {{'pk':'z'}}"),
        ),
        (
            "UPDATE",
            format!("UPDATE \"{TABLE}\" SET n = 2 WHERE pk = 'a'"),
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
            "case {case}: {statement:?} manipulates data and must run, got {status} with \
             {answered}"
        );
    }

    // What the refused statements asked for was not done: the table they named is still
    // there and still ACTIVE, and it still holds the item the seed wrote (carrying what the
    // served UPDATE set on it).
    let described = describe_when_active(&router, "the table the refusals named", TABLE).await;
    assert_eq!(
        described["TableStatus"],
        json!("ACTIVE"),
        "case the table the refusals named: it must still be ACTIVE, got {described}"
    );
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "a"}, "n": {"N": "2"}}),
        "case the table the refusals named: it must still hold its item, got {read}"
    );
}
