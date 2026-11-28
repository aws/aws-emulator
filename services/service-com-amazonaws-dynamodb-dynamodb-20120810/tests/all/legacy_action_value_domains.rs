// SPDX-License-Identifier: CC0-1.0

//! What the LEGACY `AttributeUpdates` map's actions take as their value, and how a value
//! outside that domain is refused.
//!
//! RULE: `ADD` and `DELETE` are each defined over a closed set of data types — "the
//! behavior of ADD depends on the data type of the attribute" and "If a set of values is
//! specified, then those values are subtracted from the old set"
//! (`AttributeValueUpdate$Action`) — and an entry whose `Value` is of a type its action is
//! not defined over at all is refused 400 `ValidationException` with AWS's
//! parameter-value clause, naming the ACTION the entry asked for and the `AttributeValue`
//! member tag of the value it carried:
//!
//!   `1 validation error detected: One or more parameter values were invalid: <ACTION>
//!    action is not supported for the type <TAG>`
//!
//! Both variable parts are read off the REQUEST, so the refusal is the same whatever the
//! addressed attribute holds and whether the key holds an item at all. A value the action
//! IS defined over that the STORED type cannot be combined with is the other defect, and
//! carries this service's class sentence naming neither operand
//! (`tests/all/legacy_attribute_updates_actions.rs`), while the same mistake written as an
//! `UpdateExpression` names the operator and its type set instead
//! (`tests/all/update_operand_type_sets.rs`) — the legacy map writes no operator inside a
//! text, it names an action per attribute.
//!
//! The rows below span every `AttributeValue` member (the union's own member list,
//! `smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`) against
//! both actions that take a value, plus the state of the addressed attribute and of the
//! addressed key.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: one string partition key.
const TABLE: &str = "LegacyActionValues";

/// The key of the seeded item, which carries an attribute of several types.
fn key() -> Value {
    json!({"pk": {"S": "held"}})
}

/// A key no item is stored under.
fn empty_key() -> Value {
    json!({"pk": {"S": "absent"}})
}

/// Every `AttributeValue` member, as a value of that type, beside the member TAG a
/// parameter-value clause spells it with.
fn every_attribute_type() -> Vec<(&'static str, Value)> {
    vec![
        ("S", json!({"S": "text"})),
        ("N", json!({"N": "1"})),
        ("B", json!({"B": "AQI="})),
        ("SS", json!({"SS": ["c"]})),
        ("NS", json!({"NS": ["3"]})),
        ("BS", json!({"BS": ["AwQ="]})),
        ("M", json!({"M": {"inner": {"S": "v"}}})),
        ("L", json!({"L": [{"S": "v"}]})),
        ("NULL", json!({"NULL": true})),
        ("BOOL", json!({"BOOL": true})),
    ]
}

/// The type tags each legacy action is defined over: `ADD` combines a number, a set of
/// each kind and — as the legacy member's "depends on the data type of the attribute"
/// reading — a list, while `DELETE` subtracts set members.
fn domain(action: &str) -> &'static [&'static str] {
    match action {
        "ADD" => &["N", "SS", "NS", "BS", "L"],
        _ => &["SS", "NS", "BS"],
    }
}

/// The whole message an out-of-domain value earns.
fn unsupported(action: &str, tag: &str) -> String {
    format!(
        "1 validation error detected: One or more parameter values were invalid: {action} \
         action is not supported for the type {tag}"
    )
}

/// One legacy update of `attribute`, as the request carries it.
fn legacy(attribute: &str, action: &str, value: &Value) -> Value {
    json!({attribute: {"Action": action, "Value": value}})
}

#[tokio::test]
async fn a_legacy_action_value_outside_its_domain_names_the_action_and_the_type() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "held"}, "keep": {"S": "me"}}}),
    )
    .await;

    for action in ["ADD", "DELETE"] {
        for (tag, value) in every_attribute_type() {
            let case = format!("the legacy {action} of a {tag} value");
            // The addressed attribute is one the item does not carry, so what decides
            // each row is the VALUE's own type and nothing the item holds.
            let request = json!({
                "TableName": TABLE,
                "Key": key(),
                "AttributeUpdates": legacy("fresh", action, &value),
            });
            let (status, body) = call(&router, "UpdateItem", request.clone()).await;
            if domain(action).contains(&tag) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a value the action is defined over applies, got \
                     {status} with {body}"
                );
                // Undo what an applied row left, so the next row still addresses an
                // attribute the item does not carry.
                ok(
                    &router,
                    "UpdateItem",
                    json!({"TableName": TABLE, "Key": key(),
                           "AttributeUpdates": {"fresh": {"Action": "DELETE"}}}),
                )
                .await;
                continue;
            }
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body)),
                (400, "ValidationException", unsupported(action, tag)),
                "case {case:?}: expected AWS's parameter-value clause, got {status} with \
                 {body}"
            );
        }
    }

    // Nothing any refused row asked for was written, and the seeded attribute is as it
    // was, read back through the implemented reader.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["keep"],
        json!({"S": "me"}),
        "case a refused legacy action: the seeded attribute is untouched, got {read}"
    );
    assert_eq!(
        read["Item"].get("fresh"),
        None,
        "case a refused legacy action: no attribute is left behind, got {read}"
    );
}

#[tokio::test]
async fn the_legacy_action_value_refusal_does_not_depend_on_the_addressed_item() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {
            "pk": {"S": "held"},
            "n": {"N": "5"},
            "ss": {"SS": ["a"]},
            "b": {"B": "AQI="},
        }}),
    )
    .await;

    // Each row: the case, the attribute the `ADD` addresses and the key it addresses. The
    // clause is a fact of the request, so a binary SCALAR value earns the same refusal
    // over a stored number, a stored set, a stored binary, an attribute that is not there
    // and a key that holds no item at all.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("over a stored number", "n", key()),
        ("over a stored string set", "ss", key()),
        ("over a stored binary", "b", key()),
        ("over an attribute that is not there", "fresh", key()),
        ("over a key that holds no item", "fresh", empty_key()),
    ];
    for (case, attribute, addressed) in rows {
        let (status, body) = call(
            &router,
            "UpdateItem",
            json!({
                "TableName": TABLE,
                "Key": addressed,
                "AttributeUpdates": legacy(attribute, "ADD", &json!({"B": "AQI="})),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", unsupported("ADD", "B")),
            "case a legacy ADD of a binary scalar {case}: expected AWS's parameter-value \
             clause, got {status} with {body}"
        );
    }

    // The tag is the one the REQUEST carried: a binary SET is a type the action is defined
    // over, so the same attribute takes it.
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({
            "TableName": TABLE,
            "Key": key(),
            "AttributeUpdates": legacy("bs", "ADD", &json!({"BS": ["AQI="]})),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a legacy ADD of a binary set: expected it to apply, got {status} with {body}"
    );

    // And the same mistake stated as an UPDATE EXPRESSION names the operator the text
    // wrote and the type set it takes, which is how the two generations are told apart.
    let (status, body) = call(
        &router,
        "UpdateItem",
        json!({
            "TableName": TABLE,
            "Key": key(),
            "UpdateExpression": "ADD n :v",
            "ExpressionAttributeValues": {":v": {"B": "AQI="}},
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), message(&body)),
        (
            400,
            "ValidationException",
            "1 validation error detected: Invalid UpdateExpression: Incorrect operand type \
             for operator or function; operator: ADD, operand type: Binary, typeSet: \
             ALLOWED_FOR_ADD_OPERAND"
                .to_string()
        ),
        "case an expression ADD of a binary scalar: expected the operand-type clause, got \
         {status} with {body}"
    );

    // Every refused row left the stored values where they were.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        (read["Item"]["n"].clone(), read["Item"]["b"].clone()),
        (json!({"N": "5"}), json!({"B": "AQI="})),
        "case a refused legacy ADD: the stored values are untouched, got {read}"
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
        "case a refused legacy ADD: no item is created, got {absent}"
    );
}
