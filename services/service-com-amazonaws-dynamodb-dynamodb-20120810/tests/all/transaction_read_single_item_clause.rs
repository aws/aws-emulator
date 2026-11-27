// SPDX-License-Identifier: CC0-1.0

//! What a `SELECT` inside `ExecuteTransaction` may state in its `WHERE` clause, driven
//! end to end.
//!
//! RULE under test: a transactional read reports the ONE item its statement addresses, so
//! the clause must pin every key attribute by equality AND STATE NOTHING ELSE — a term
//! that is no such equality (a boolean function, a comparison that is not `=`, a range, a
//! membership test, a branch of an `OR`) narrows a result set instead of addressing an
//! item. Such a statement is refused, before anything is read, with HTTP 400
//! `ValidationException` whose message states the offending statement's slot in
//! `TransactStatements` followed by this operation's fixed sentence — never a 200
//! reporting whatever the clause happened to match, which for a clause that pins no whole
//! key can be more than the single item the slot is guaranteed to hold.
//!
//! The rows differ along what the rule generalises over: the KIND of term beside the key
//! equalities (every predicate form the dialect parses), the SLOT the offending statement
//! was sent in (which the message reports), the table's key schema (a partition key alone
//! and a composite key), and which half of a composite key the clause leaves free.
//!
//! The CONTROL drives the same transaction with clauses that state the key equalities and
//! nothing else — on both key schemas, and with a projection — so no row passes by
//! refusing every transactional read: each is served, reports the addressed item, and a
//! key that holds no item reports no item rather than a refusal.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table addressed by the rows about a partition key alone.
const TABLE: &str = "TransactionReadClause";

/// The table addressed by the rows about a composite key.
const COMPOSITE: &str = "TransactionReadComposite";

/// The fixed sentence this operation states the single-item read rule with.
const SINGLE_ITEM: &str =
    "Select statements within ExecuteTransaction must specify the primary key in the where clause.";

/// The emulator with both tables and one item in each.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({"TableName": TABLE, "BillingMode": "PAY_PER_REQUEST",
               "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
               "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}]}),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        json!({"TableName": COMPOSITE, "BillingMode": "PAY_PER_REQUEST",
               "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                             {"AttributeName": "sk", "KeyType": "RANGE"}],
               "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                        {"AttributeName": "sk", "AttributeType": "S"}]}),
    )
    .await;
    describe_when_active(&router, TABLE, TABLE).await;
    describe_when_active(&router, COMPOSITE, COMPOSITE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "k1"}, "note": {"S": "beta"}, "n": {"N": "3"}}}),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": COMPOSITE,
               "Item": {"pk": {"S": "k1"}, "sk": {"S": "a"}, "note": {"S": "beta"}}}),
    )
    .await;
    router
}

/// The message a defect of the statement in `slot` is reported under: the slot it was
/// sent in, then the sentence the rule is stated with.
fn in_slot(slot: usize) -> String {
    format!("Validation failed in TransactStatements[{slot}]: {SINGLE_ITEM}")
}

/// Drive one transaction that must be refused as the request's own defect, answering the
/// message it was refused with.
async fn refused(router: &MultiServiceRouter, case: &str, statements: Value) -> String {
    let (status, answered) = call(
        router,
        "ExecuteTransaction",
        json!({"TransactStatements": statements}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
    );
    message(&answered)
}

/// Every predicate form a `WHERE` clause can state BESIDE the key equalities, each a term
/// that addresses no single item. One row per form the dialect parses.
fn terms_beside_the_key() -> Vec<(&'static str, &'static str)> {
    vec![
        ("an existence test", "attribute_exists(note)"),
        ("an absence test", "attribute_not_exists(gone)"),
        ("a type test", "attribute_type(note, 'S')"),
        ("a prefix test", "begins_with(note, 'bet')"),
        ("a substring test", "contains(note, 'et')"),
        ("a comparison that is not equality", "n > 1"),
        ("an inequality", "note <> 'zzz'"),
        ("a range", "n BETWEEN 1 AND 9"),
        ("a null test", "note IS NOT NULL"),
    ]
}

/// RULE: a transactional read whose clause states anything beside the key equalities is
/// refused, whichever term it states — the full primary key being pinned is not enough.
#[tokio::test]
async fn a_transactional_read_refuses_every_term_beside_the_key_equalities() {
    let router = seeded().await;

    for (case, term) in terms_beside_the_key() {
        let statement = format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'k1' AND {term}");
        let stated = refused(&router, case, json!([{"Statement": statement}])).await;
        assert_eq!(
            stated,
            in_slot(0),
            "case {case:?} ({statement:?}): expected {:?}, got {stated:?}",
            in_slot(0)
        );
    }

    // An `OR` requires none of its branches, so a clause that offers two keys addresses
    // no one item either — the same refusal.
    let statement = format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'k1' OR pk = 'k2'");
    let stated = refused(
        &router,
        "a disjunction of two keys",
        json!([{"Statement": statement}]),
    )
    .await;
    assert_eq!(
        stated,
        in_slot(0),
        "case a disjunction of two keys: expected {:?}, got {stated:?}",
        in_slot(0)
    );
}

/// RULE: the refusal reports the SLOT the offending statement was sent in, so the same
/// defect in a later slot names that slot.
#[tokio::test]
async fn the_refusal_names_the_slot_the_offending_statement_was_sent_in() {
    let router = seeded().await;

    let offending = json!({"Statement":
        format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'k1' AND attribute_exists(note)")});

    for slot in [0usize, 1, 2] {
        // The statements beside the offending one are well formed, and each addresses its
        // own key — a transaction may not name one item twice — so the refusal is about
        // the clause under test and not about the company it travels in.
        let mut statements: Vec<Value> = (0..2)
            .map(|at| {
                json!({"Statement":
                    format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'spare-{at}'")})
            })
            .collect();
        statements.insert(slot, offending.clone());
        let case = format!("the offending statement in slot {slot}");
        let stated = refused(&router, &case, json!(statements)).await;
        assert_eq!(
            stated,
            in_slot(slot),
            "case {case:?}: expected {:?}, got {stated:?}",
            in_slot(slot)
        );
    }
}

/// RULE: a clause that leaves a key attribute free addresses no single item either, so a
/// composite-key table is read only by a clause pinning BOTH halves — and the same
/// sentence reports a clause that pins one half and narrows with another term.
#[tokio::test]
async fn a_composite_key_is_read_only_by_a_clause_pinning_both_halves() {
    let router = seeded().await;

    for (case, clause) in [
        ("only the partition key", "pk = 'k1'"),
        ("only the sort key", "sk = 'a'"),
        (
            "the partition key beside a sort-key range",
            "pk = 'k1' AND sk BETWEEN 'a' AND 'z'",
        ),
        (
            "the partition key beside a sort-key prefix",
            "pk = 'k1' AND begins_with(sk, 'a')",
        ),
        (
            "both halves beside an existence test",
            "pk = 'k1' AND sk = 'a' AND attribute_exists(note)",
        ),
    ] {
        let statement = format!("SELECT * FROM \"{COMPOSITE}\" WHERE {clause}");
        let stated = refused(&router, case, json!([{"Statement": statement}])).await;
        assert_eq!(
            stated,
            in_slot(0),
            "case {case:?} ({statement:?}): expected {:?}, got {stated:?}",
            in_slot(0)
        );
    }
}

/// The CONTROL for every refusal above: a clause that states the key equalities and
/// nothing else is SERVED on both key schemas, reports the addressed item, and reports no
/// item for a key that holds none.
#[tokio::test]
async fn a_clause_that_states_the_key_equalities_alone_is_served() {
    let router = seeded().await;

    let answered = ok(
        &router,
        "ExecuteTransaction",
        json!({"TransactStatements": [
            {"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'k1'")},
            {"Statement": format!("SELECT note FROM \"{TABLE}\" WHERE 'spare-0' = pk")},
            {"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'absent'")},
            {"Statement": format!("SELECT * FROM \"{COMPOSITE}\" WHERE pk = 'k1' AND sk = 'a'")},
        ]}),
    )
    .await;

    let responses = answered["Responses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        responses.len(),
        4,
        "case a served read transaction: one response per statement, got {answered}"
    );
    assert_eq!(
        responses[0]["Item"]["note"],
        json!({"S": "beta"}),
        "case the whole item: expected the addressed item, got {answered}"
    );
    assert_eq!(
        responses[1].get("Item"),
        None,
        "case a projection over a key that holds no item: expected no item, got {answered}"
    );
    assert_eq!(
        responses[2].get("Item"),
        None,
        "case a key that holds no item: expected no item reported, got {answered}"
    );
    assert_eq!(
        responses[3]["Item"]["note"],
        json!({"S": "beta"}),
        "case the composite key: expected the addressed item, got {answered}"
    );

    // A projection over a key that DOES hold an item reports exactly the attributes the
    // statement named, so the served rows above are reads and not empty responses.
    let answered = ok(
        &router,
        "ExecuteTransaction",
        json!({"TransactStatements": [
            {"Statement": format!("SELECT note FROM \"{TABLE}\" WHERE 'k1' = pk")},
        ]}),
    )
    .await;
    assert_eq!(
        answered["Responses"][0]["Item"],
        json!({"note": {"S": "beta"}}),
        "case a projected read written value-first: expected the projection, got {answered}"
    );
}
