// SPDX-License-Identifier: CC0-1.0

//! What the PartiQL surface does with the text AROUND a statement: the verb it has to
//! open with, the name it may not bind to the table it addresses, the `;` that ends the
//! one query a request states, and a quoted run the text never closes.
//!
//! Two rules, each asserted over the forms it generalises over rather than over one
//! statement:
//!
//! 1. This surface runs statements that read or write items, so text that opens with
//!    none of the four verbs manipulates no data: text that reads as ONE complete
//!    expression — the language's other statement form, a query — states no data
//!    manipulation, and text with a token left over past that expression stopped at that
//!    token. A statement that binds a name to the table it addresses — with `AS` or
//!    without — is refused as aliasing, which the surface does not serve.
//! 2. A statement that does not parse is refused with ONE fixed sentence whose tail is
//!    the parser's phrase for the position the parse stopped at: the `FROM` a projection
//!    list must be followed by, text past the `;` that ends the query, a clause that
//!    stops before its predicate, and — where the text simply stops, inside a quoted run
//!    — no phrase at all.
//!
//! Every row is sent through `ExecuteStatement`; the two surfaces that carry the same
//! statement (`BatchExecuteStatement` and `ExecuteTransaction`) are covered by
//! `partiql_refusal_wording`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses.
const TABLE: &str = "items";

/// The fixed sentence a parse defect is stated with, over the parser's phrase for the
/// position the parse stopped at. A defect the parser reports no phrase for states the
/// sentence's fixed text and nothing after it.
fn parse_failure(reason: &str) -> String {
    format!("Statement wasn't well formed, can't be processed: {reason}")
}

/// The sentence a statement that binds a name to its table is refused with.
const ALIASING: &str = "Aliasing is not supported";

/// The parser's phrase for text that read as one complete expression and manipulates no
/// data, and for text a token is left over past that expression in.
const NO_MANIPULATION: &str = "Expected data manipulation";
const TRAILING_TOKEN: &str = "Unexpected token after expression";

/// An emulator holding one table with one item in it, so a statement that IS served can
/// be seen to have run.
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

/// Run one statement and report its HTTP status and body.
async fn run(router: &MultiServiceRouter, statement: &str) -> (u16, Value) {
    call(
        router,
        "ExecuteStatement",
        json!({"Statement": statement}),
    )
    .await
}

/// Run one statement that must be refused, and report the message it was refused with.
async fn refused(router: &MultiServiceRouter, case: &str, statement: &str) -> String {
    let (status, answered) = run(router, statement).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400 for {statement:?}, got {status} with {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ValidationException",
        "case {case}: expected ValidationException for {statement:?}, got {answered}"
    );
    message(&answered)
}

#[tokio::test]
async fn text_that_states_no_data_manipulation_and_a_bound_table_name_are_refused_as_such() {
    let router = seeded().await;

    // The rows differ in WHAT the text reads as: a bare name, a quoted name, a literal and
    // a predicate each read as one complete expression and nothing more, so each states no
    // data manipulation; a misspelled verb, a bare name with a word after it and a
    // statement of another dialect each read as a name with a token left over, which is
    // where the parse stopped.
    for (case, statement, reason) in [
        ("a bare name alone", TABLE.to_string(), NO_MANIPULATION),
        (
            "a quoted name alone",
            format!("\"{TABLE}\""),
            NO_MANIPULATION,
        ),
        (
            "a string literal alone",
            "'not a statement'".to_string(),
            NO_MANIPULATION,
        ),
        ("a predicate alone", "pk = 'a'".to_string(), NO_MANIPULATION),
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
            "a bare name and a word after it",
            format!("{TABLE} rows"),
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
    ] {
        let stated = refused(&router, case, &statement).await;
        assert_eq!(
            stated,
            parse_failure(reason),
            "case {case}: {statement:?} manipulates no data, got {stated:?}"
        );
    }

    // The rows differ in WHICH verb binds the name, in whether the binding spells `AS`,
    // and in whether the bound name is bare or quoted — each is the same aliasing.
    for (case, statement) in [
        ("a SELECT source bound without AS", format!("SELECT * FROM \"{TABLE}\" t")),
        (
            "a SELECT source bound with AS",
            format!("SELECT * FROM \"{TABLE}\" AS t"),
        ),
        (
            "a SELECT source bound to a quoted name",
            format!("SELECT * FROM \"{TABLE}\" \"t\""),
        ),
        (
            "a SELECT source bound before a WHERE clause",
            format!("SELECT * FROM \"{TABLE}\" t WHERE t.pk = 'a'"),
        ),
        (
            "a DELETE source bound without AS",
            format!("DELETE FROM \"{TABLE}\" t WHERE pk = 'a'"),
        ),
        (
            "an UPDATE target bound with AS",
            format!("UPDATE \"{TABLE}\" AS t SET n = 2 WHERE pk = 'a'"),
        ),
        (
            "an INSERT target bound without AS",
            format!("INSERT INTO \"{TABLE}\" t VALUE {{'pk':'z'}}"),
        ),
    ] {
        let stated = refused(&router, case, &statement).await;
        assert_eq!(
            stated, ALIASING,
            "case {case}: {statement:?} binds a name to the table, got {stated:?}"
        );
    }

    // The same statements WITHOUT a binding are served, so the refusals above are about
    // the binding and not about the clause that carried it.
    for (case, statement) in [
        ("an unbound SELECT", format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a'")),
        (
            "an unbound UPDATE",
            format!("UPDATE \"{TABLE}\" SET n = 2 WHERE pk = 'a'"),
        ),
        (
            "an unbound INSERT",
            format!("INSERT INTO \"{TABLE}\" VALUE {{'pk':'z'}}"),
        ),
        (
            "an unbound DELETE",
            format!("DELETE FROM \"{TABLE}\" WHERE pk = 'z'"),
        ),
    ] {
        let (status, answered) = run(&router, &statement).await;
        assert_eq!(
            status, 200,
            "case {case}: {statement:?} names no alias and must be served, got {status} with \
             {answered}"
        );
    }
}

#[tokio::test]
async fn each_parse_position_states_the_parsers_own_phrase_for_it() {
    let router = seeded().await;

    // One row per POSITION a parse can stop at, each spelled more than one way where the
    // position admits it: the phrase is the parser's for that position, and the sentence
    // around it is fixed.
    for (case, statement, reason) in [
        (
            "a projection list with no FROM after it",
            format!("SELECT pk, n \"{TABLE}\""),
            "Expected FROM after SELECT list",
        ),
        (
            "a star projection with no FROM after it",
            format!("SELECT * {TABLE}"),
            "Expected FROM after SELECT list",
        ),
        (
            "a second semicolon after the query",
            format!("SELECT * FROM \"{TABLE}\";;"),
            "Unexpected token after semicolon. (Only one query is allowed.)",
        ),
        (
            "a second query after the semicolon",
            format!("SELECT * FROM \"{TABLE}\"; SELECT * FROM \"{TABLE}\""),
            "Unexpected token after semicolon. (Only one query is allowed.)",
        ),
        (
            "a word after the semicolon",
            format!("DELETE FROM \"{TABLE}\" WHERE pk = 'a'; junk"),
            "Unexpected token after semicolon. (Only one query is allowed.)",
        ),
        (
            "a token after a complete statement",
            format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a' junk"),
            "Unexpected token after expression",
        ),
        (
            "a clause that stops before its predicate",
            format!("SELECT * FROM \"{TABLE}\" WHERE"),
            "Unexpected term",
        ),
        (
            "a single-quoted literal the text never closes",
            format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'abc"),
            "",
        ),
        (
            "a quoted name the text never closes",
            format!("SELECT * FROM \"{TABLE}"),
            "",
        ),
        (
            "an unclosed literal in a written value",
            format!("INSERT INTO \"{TABLE}\" VALUE {{'pk': 'z}}"),
            "",
        ),
    ] {
        let stated = refused(&router, case, &statement).await;
        assert_eq!(
            stated,
            parse_failure(reason),
            "case {case}: {statement:?} stops at a position the parser phrases as {reason:?}, \
             got {stated:?}"
        );
    }

    // "Only one query is allowed": ONE semicolon ends that query, so a statement closed
    // by one is served — the refusals above are about what follows it.
    for (case, statement) in [
        ("a read closed by a semicolon", format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'a';")),
        (
            "a write closed by a semicolon",
            format!("UPDATE \"{TABLE}\" SET n = 3 WHERE pk = 'a';"),
        ),
    ] {
        let (status, answered) = run(&router, &statement).await;
        assert_eq!(
            status, 200,
            "case {case}: {statement:?} states one query and must be served, got {status} with \
             {answered}"
        );
    }

    // The served write above landed, so a statement a semicolon closes is run rather
    // than merely accepted.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["n"],
        json!({"N": "3"}),
        "case a write closed by a semicolon: expected the stored item to carry the written \
         value, got {read}"
    );
}
