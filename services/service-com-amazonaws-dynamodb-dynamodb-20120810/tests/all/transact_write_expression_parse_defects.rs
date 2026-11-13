// SPDX-License-Identifier: CC0-1.0

//! What a TRANSACTIONAL write answers when one of its actions carries an expression the
//! expression language cannot accept, driven end to end.
//!
//! RULE under test: a PARSE-level defect in the `ConditionExpression` or
//! `UpdateExpression` of ANY `TransactItems` action is a defect of the REQUEST, so the
//! whole call is refused with a top-level `ValidationException` (HTTP 400) carrying no
//! `CancellationReasons` — the class `TransactionCanceledException` models is per-action
//! cancellation and not a malformed request — and NOTHING the transaction asked for is
//! written, the healthy sibling actions included. The sentence is the one the SINGLE-ITEM
//! operation states for the same expression: one defect, one wording, whichever operation
//! carried it, stated on its own because an action's expression members are members of
//! that action rather than of the request
//! (`element_expression_envelopes` spans where that envelope belongs).
//!
//! The defects spanned are the three kinds a parse can find: a DynamoDB reserved word
//! written out as an attribute name (the reserved-words list in the Amazon DynamoDB
//! Developer Guide, which `ExpressionAttributeNames` exists to work around), an expression
//! the grammar cannot complete, and a pair of parentheses that groups what is already
//! grouped. The rows cross them with the action kind (`Update`, `Put`, `Delete`,
//! `ConditionCheck`), the member the defect sat in (`UpdateExpression`,
//! `ConditionExpression`) and where the faulty action sits among its siblings (first,
//! second) — each row pairing the faulty action with a healthy `Put` that must not land.
//!
//! Every call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row below drives.
const TABLE: &str = "TransactExpressionDefects";

/// The envelope a refusal about a REQUEST's own members is stated inside, which the
/// sentence an ACTION's member earns is stated outside of.
const ENVELOPE: &str = "1 validation error detected: ";

/// The key of the seeded item every faulty action addresses, so no row is refused for its
/// key instead of for its expression.
fn seed_key() -> Value {
    json!({"pk": {"S": "seed"}})
}

/// The key the healthy sibling `Put` writes, which nothing seeds: a refused transaction
/// must leave it unwritten.
fn side_key() -> Value {
    json!({"pk": {"S": "side"}})
}

/// The healthy action every row pairs its faulty one with: a `Put` of an item nothing else
/// touches, which a refused transaction must not write.
fn healthy_put() -> Value {
    json!({"Put": {
        "TableName": TABLE,
        "Item": {"pk": {"S": "side"}, "val": {"S": "healthy"}}}})
}

/// Seed the table and the item the faulty actions address.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "seed"}, "status": {"S": "active"}, "foo": {"S": "orig"}}}),
    )
    .await;
    router
}

/// The item each addressed key holds, for the assertions that a refused transaction wrote
/// nothing.
async fn stored(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<Value> {
    let mut items = Vec::new();
    for key in [seed_key(), side_key()] {
        let read = ok(
            router,
            "GetItem",
            json!({"TableName": TABLE, "Key": key, "ConsistentRead": true}),
        )
        .await;
        items.push(read.get("Item").cloned().unwrap_or(Value::Null));
    }
    items
}

/// One row: the defect, the action that carries it, and the single-item request that
/// carries the SAME expression — the reference the refusal's wording is read from.
struct Row {
    case: &'static str,
    /// The faulty action, and the healthy `Put` it is paired with, in request order.
    transact_items: Value,
    /// The single-item operation and request carrying the same expression.
    single_operation: &'static str,
    single_request: Value,
}

/// The rows: three parse defects, across the action kinds, the two expression members and
/// both positions among the siblings.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an Update whose UpdateExpression writes out a reserved word",
            transact_items: json!([
                healthy_put(),
                {"Update": {
                    "TableName": TABLE,
                    "Key": seed_key(),
                    "UpdateExpression": "SET status = :v",
                    "ExpressionAttributeValues": {":v": {"S": "active"}}}}]),
            single_operation: "UpdateItem",
            single_request: json!({
                "TableName": TABLE,
                "Key": seed_key(),
                "UpdateExpression": "SET status = :v",
                "ExpressionAttributeValues": {":v": {"S": "active"}}}),
        },
        Row {
            case: "a ConditionCheck whose ConditionExpression writes out a reserved word",
            transact_items: json!([
                {"ConditionCheck": {
                    "TableName": TABLE,
                    "Key": seed_key(),
                    "ConditionExpression": "status = :v",
                    "ExpressionAttributeValues": {":v": {"S": "active"}}}},
                healthy_put()]),
            single_operation: "PutItem",
            single_request: json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "seed"}, "status": {"S": "paused"}},
                "ConditionExpression": "status = :v",
                "ExpressionAttributeValues": {":v": {"S": "active"}}}),
        },
        Row {
            case: "an Update whose UpdateExpression stops before its value",
            transact_items: json!([
                healthy_put(),
                {"Update": {
                    "TableName": TABLE,
                    "Key": seed_key(),
                    "UpdateExpression": "SET foo ="}}]),
            single_operation: "UpdateItem",
            single_request: json!({
                "TableName": TABLE,
                "Key": seed_key(),
                "UpdateExpression": "SET foo ="}),
        },
        Row {
            case: "a Delete whose ConditionExpression groups what is already grouped",
            transact_items: json!([
                {"Delete": {
                    "TableName": TABLE,
                    "Key": seed_key(),
                    "ConditionExpression": "((pk = :k))",
                    "ExpressionAttributeValues": {":k": {"S": "seed"}}}},
                healthy_put()]),
            single_operation: "DeleteItem",
            single_request: json!({
                "TableName": TABLE,
                "Key": seed_key(),
                "ConditionExpression": "((pk = :k))",
                "ExpressionAttributeValues": {":k": {"S": "seed"}}}),
        },
        Row {
            case: "a Put whose ConditionExpression groups what is already grouped, deeper",
            transact_items: json!([
                healthy_put(),
                {"Put": {
                    "TableName": TABLE,
                    "Item": {"pk": {"S": "seed"}, "status": {"S": "paused"}},
                    "ConditionExpression": "(((attribute_exists(pk))))",
                    }}]),
            single_operation: "PutItem",
            single_request: json!({
                "TableName": TABLE,
                "Item": {"pk": {"S": "seed"}, "status": {"S": "paused"}},
                "ConditionExpression": "(((attribute_exists(pk))))"}),
        },
        Row {
            case: "an Update whose UpdateExpression names an unknown function",
            transact_items: json!([
                healthy_put(),
                {"Update": {
                    "TableName": TABLE,
                    "Key": seed_key(),
                    "UpdateExpression": "SET foo = if_exists(bar, :v)",
                    "ExpressionAttributeValues": {":v": {"S": "x"}}}}]),
            single_operation: "UpdateItem",
            single_request: json!({
                "TableName": TABLE,
                "Key": seed_key(),
                "UpdateExpression": "SET foo = if_exists(bar, :v)",
                "ExpressionAttributeValues": {":v": {"S": "x"}}}),
        },
    ]
}

#[tokio::test]
async fn a_parse_defect_in_any_action_refuses_the_whole_transaction() {
    let router = seeded().await;
    let before = stored(&router).await;

    for row in rows() {
        let case = row.case;
        // The sentence the SINGLE-ITEM operation states for the same expression, which is
        // the sentence the transaction must state too — without the envelope, because an
        // action's expression member is a member of that action.
        let (single_status, single_body) =
            call(&router, row.single_operation, row.single_request.clone()).await;
        assert_eq!(
            (single_status, error_code(&single_body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: the single-item {} must refuse the same expression, got \
             {single_status} with {single_body}",
            row.single_operation
        );
        let single = message(&single_body);
        let expected = single
            .strip_prefix(ENVELOPE)
            .map(str::to_string)
            .unwrap_or_else(|| single.clone());

        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": row.transact_items.clone()}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the whole transaction refused with a top-level \
             ValidationException, got {status} with {body}"
        );
        assert_eq!(
            body.get("CancellationReasons").cloned(),
            None,
            "case {case:?}: a malformed request carries no per-action cancellation, got {body}"
        );
        let said = message(&body);
        assert_eq!(
            said, expected,
            "case {case:?}: expected the single-item sentence {expected:?}, got {said:?}"
        );
    }

    // Nothing any refused transaction asked for was written: the healthy sibling `Put`
    // never landed, and the seeded item is as it was.
    let after = stored(&router).await;
    assert_eq!(
        after, before,
        "a refused transaction writes nothing: expected {before:?}, got {after:?}"
    );
    assert_eq!(
        after[1],
        Value::Null,
        "the healthy sibling Put of a refused transaction never lands, got {:?}",
        after[1]
    );
}

#[tokio::test]
async fn a_transaction_whose_expressions_parse_still_lands() {
    // The positive control the rule above is about: the same shapes of action, with
    // expressions the language accepts — the reserved word reached through
    // `ExpressionAttributeNames`, the update completed, the parentheses grouping something
    // — apply, so the refusals above come from the expressions and not from the actions
    // being unserved.
    let router = seeded().await;
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [
            healthy_put(),
            {"Update": {
                "TableName": TABLE,
                "Key": seed_key(),
                "UpdateExpression": "SET #s = :v",
                "ConditionExpression": "(pk = :k) AND (attribute_exists(foo))",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":v": {"S": "paused"},
                                              ":k": {"S": "seed"}}}}]}),
    )
    .await;

    let items = stored(&router).await;
    assert_eq!(
        items[0]["status"],
        json!({"S": "paused"}),
        "the update of the transaction that parsed applied, got {:?}",
        items[0]
    );
    assert_eq!(
        items[1]["val"],
        json!({"S": "healthy"}),
        "the sibling Put of the transaction that parsed landed, got {:?}",
        items[1]
    );
}
