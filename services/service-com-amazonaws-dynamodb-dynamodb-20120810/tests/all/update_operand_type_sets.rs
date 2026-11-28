// SPDX-License-Identifier: CC0-1.0

//! An `UpdateExpression` operand of a type its OPERATOR cannot take, over every
//! operator that takes a value operand and every type one can be given.
//!
//! The rule: a value operand outside the closed set of types its operator accepts is
//! refused 400 `ValidationException` naming the operator, the operand's own type and the
//! name of that type set — a statement about the REQUEST, so it does not depend on what
//! the item holds at the path, on whether the path holds anything at all, or on which
//! operation carried the expression.
//!
//! The two domains come from the model's own doc for that member
//! (`smithy_query(query="doc", shape="com.amazonaws.dynamodb#UpdateItemInput",
//! member="UpdateExpression")`): "The ADD action only supports Number and set data
//! types" and "The DELETE action only supports set data types". The type names the
//! message spells are the `AttributeValue` members' own type names, one capitalised word
//! per part of the documented type
//! (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`: "An
//! attribute of type String Set" for `SS` is `StringSet`, `B` is `Binary`, `L` is
//! `List`). The TYPE SET the message names is the domain of the OPERATOR that refused the
//! operand, so a caller is pointed at the rule it broke rather than at the other
//! operator's.
//!
//! Driven as a TABLE over the operators, every `AttributeValue` type (including the ones
//! each operator DOES take, which must be served or refused for the stored type
//! instead), the state of the addressed path, and the two surfaces that read an
//! `UpdateExpression` — `UpdateItem` and `TransactWriteItems` share one expression
//! reader, so the clause cannot differ between them.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: one string partition key.
const TABLE: &str = "Operands";

/// The seeded item's key — the path rows that need an existing attribute address.
fn key() -> Value {
    json!({"pk": {"S": "held"}})
}

/// A key no item is stored under, for the rows whose path holds nothing.
fn empty_key() -> Value {
    json!({"pk": {"S": "absent"}})
}

/// The table with one item carrying an attribute of every type an operand could be
/// combined with.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {
            "pk": {"S": "held"},
            "n": {"N": "5"},
            "ss": {"SS": ["a", "b"]},
            "ns": {"NS": ["1", "2"]},
            "bs": {"BS": ["AQI="]},
        }}),
    )
    .await;
    router
}

/// Every `AttributeValue` type, as a value of that type: the closure the operand rules
/// are tested over, from the union's own member list
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`), each
/// beside the type NAME the operand-type refusal spells it with.
fn every_attribute_type() -> Vec<(&'static str, Value)> {
    vec![
        ("String", json!({"S": "text"})),
        ("Number", json!({"N": "1"})),
        ("Binary", json!({"B": "AQI="})),
        ("StringSet", json!({"SS": ["c"]})),
        ("NumberSet", json!({"NS": ["3"]})),
        ("BinarySet", json!({"BS": ["AwQ="]})),
        ("Map", json!({"M": {"inner": {"S": "v"}}})),
        ("List", json!({"L": [{"S": "v"}]})),
        ("Null", json!({"NULL": true})),
        ("Boolean", json!({"BOOL": true})),
    ]
}

/// The whole message AWS states for an operand outside an operator's domain.
fn expected_message(operator: &str, operand_type: &str, type_set: &str) -> String {
    format!(
        "1 validation error detected: Invalid UpdateExpression: Incorrect operand type for \
         operator or function; operator: {operator}, operand type: {operand_type}, typeSet: \
         {type_set}"
    )
}

/// AWS's fixed sentence for the OTHER operand defect: an operand of a type the operator
/// takes that the type the item holds cannot be combined with. It names neither operand,
/// and `tests/all/validation_message_fidelity.rs` is the suite that spans it; it is
/// stated here only to pin that an in-domain operand never earns the message above.
const CLASS_SENTENCE: &str = "An operand in the update expression has an incorrect data type";

/// Call `operation`, require 400 `ValidationException`, and answer the message.
async fn refusal(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request.clone()).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation} {request}): expected 400 ValidationException, got {status} \
         with {body}"
    );
    message(&body)
}

/// One operator that takes a value operand: its keyword, the NAME of the domain its
/// refusal states, and the type names it accepts.
struct Operator {
    keyword: &'static str,
    type_set: &'static str,
    accepts: &'static [&'static str],
}

/// The two update operators whose operand is a value, with the domains the model's
/// `UpdateExpression` doc states for them and the name each one's refusal gives its own
/// domain.
fn operators() -> Vec<Operator> {
    vec![
        Operator {
            keyword: "ADD",
            type_set: "ALLOWED_FOR_ADD_OPERAND",
            accepts: &["Number", "StringSet", "NumberSet", "BinarySet"],
        },
        Operator {
            keyword: "DELETE",
            type_set: "ALLOWED_FOR_DELETE_OPERAND",
            accepts: &["StringSet", "NumberSet", "BinarySet"],
        },
    ]
}

#[tokio::test]
async fn an_operand_outside_its_operator_type_set_names_the_operator_and_the_set() {
    let router = seeded().await;

    for operator in operators() {
        for (type_name, operand) in every_attribute_type() {
            // The path is one the operator COULD be applied to — the seeded number for
            // `ADD`, the seeded string set for both — so a row that is refused is
            // refused for its operand's type and for nothing else.
            let path = if operator.keyword == "ADD" && type_name == "Number" {
                "n"
            } else {
                "ss"
            };
            let case = format!("{} {path} an operand of type {type_name}", operator.keyword);
            let request = json!({
                "TableName": TABLE,
                "Key": key(),
                "UpdateExpression": format!("{} {path} :v", operator.keyword),
                "ExpressionAttributeValues": {":v": operand},
            });
            if operator.accepts.contains(&type_name) {
                // A type the operator DOES take is not this refusal: it either applies
                // or, where the type the item holds cannot be combined with it, states
                // the class sentence that names neither operand.
                let (status, body) = call(&router, "UpdateItem", request.clone()).await;
                let stated = message(&body);
                assert!(
                    status == 200 || (status == 400 && stated == CLASS_SENTENCE),
                    "case {case:?}: an operand the operator takes either applies or states \
                     the class sentence, got {status} with {body}"
                );
                continue;
            }
            let stated = refusal(&router, &case, "UpdateItem", request).await;
            assert_eq!(
                stated,
                expected_message(operator.keyword, type_name, operator.type_set),
                "case {case:?}: expected AWS's operand-type message, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_does_not_depend_on_the_item_or_the_operation() {
    let router = seeded().await;

    // Each row: the case, the path the `ADD` addresses, the key it addresses, and the
    // operation that carries the expression. The rule is about the request, so the
    // message is the same whether the path holds a number, a set, nothing at all, or
    // sits behind a key no item is stored under — and the same whichever operation read
    // the expression.
    let add_string = expected_message("ADD", "String", "ALLOWED_FOR_ADD_OPERAND");
    let rows: Vec<(&str, Value)> = vec![
        (
            "UpdateItem over a stored number",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "ADD n :v",
                   "ExpressionAttributeValues": {":v": {"S": "c"}}}),
        ),
        (
            "UpdateItem over a stored string set",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "ADD ss :v",
                   "ExpressionAttributeValues": {":v": {"S": "c"}}}),
        ),
        (
            "UpdateItem over an attribute that is not there",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "ADD fresh :v",
                   "ExpressionAttributeValues": {":v": {"S": "c"}}}),
        ),
        (
            "UpdateItem over a key that holds no item",
            json!({"TableName": TABLE, "Key": empty_key(),
                   "UpdateExpression": "ADD fresh :v",
                   "ExpressionAttributeValues": {":v": {"S": "c"}}}),
        ),
    ];

    for (case, request) in rows {
        let stated = refusal(&router, case, "UpdateItem", request).await;
        assert_eq!(
            stated, add_string,
            "case {case:?}: expected AWS's operand-type message, got {stated:?}"
        );
    }

    // The same expression inside a transaction is read by the same expression reader, so
    // it states the same clause — carried by the CANCELLATION REASON of the action that
    // stated it, which is how a transaction reports a failure its own action earned, which
    // is why the clause is matched rather than the whole message.
    let (status, body) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": {
            "TableName": TABLE,
            "Key": key(),
            "UpdateExpression": "ADD n :v",
            "ExpressionAttributeValues": {":v": {"S": "c"}},
        }}]}),
    )
    .await;
    let stated = body["CancellationReasons"][0]["Message"]
        .as_str()
        .unwrap_or("<no reason>")
        .to_string();
    assert_eq!(
        (
            status,
            error_code(&body).as_str(),
            body["CancellationReasons"][0]["Code"].as_str(),
        ),
        (400, "TransactionCanceledException", Some("ValidationError")),
        "case a transactional ADD of a string: expected the action cancelled, got {status} \
         with {body}"
    );
    assert!(
        stated.contains(
            "Incorrect operand type for operator or function; operator: ADD, operand type: \
             String, typeSet: ALLOWED_FOR_ADD_OPERAND"
        ),
        "case a transactional ADD of a string: expected AWS's operand-type clause, got {stated:?}"
    );

    // Nothing was written by any refused row: the seeded item is exactly as it was, and
    // the key that held no item still holds none.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["n"],
        json!({"N": "5"}),
        "case a refused ADD: the stored number is untouched, got {read}"
    );
    assert_eq!(
        read["Item"].get("fresh"),
        None,
        "case a refused ADD: no attribute is created, got {read}"
    );
    let absent = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": empty_key(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        absent.get("Item"),
        None,
        "case a refused ADD: no item is created, got {absent}"
    );
}
