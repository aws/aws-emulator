// SPDX-License-Identifier: CC0-1.0

//! How a `TransactWriteItems` whose action list is past its cap QUOTES the actions it was
//! sent.
//!
//! THE RULE: a list member carrying more elements than the `@length` maximum the model
//! declares for it (`smithy_query(query="shape",
//! shape="com.amazonaws.dynamodb#TransactWriteItemList")` is `@length{min:1,max:100}`) is
//! refused HTTP 400 `ValidationException` whose message is the envelope that counts the
//! clauses over one clause, and for the member a transactional WRITE states its actions in
//! that clause QUOTES the submitted list: the object rendering this service states a refused
//! structure value in, where every structure member is spelled out in the order the model
//! declares it, every member the request left out renders as the unset field it is, and every
//! attribute value is written over every field of the service's own attribute-value object —
//! the arm the value carries filled from the request and the other fifteen `null`. The member
//! path and the bound follow it, exactly as they do for a list this clause quotes nothing of.
//!
//! The rows span what the rule generalises over: WHICH action the quoted element states
//! (`Put`, `Delete`, `Update`, `ConditionCheck`), which of that action's optional members the
//! request filled (a condition, both substitution tables, what to return on a failed
//! condition — and none of them), and WHICH datatype each attribute value carries (a string,
//! a number, a blob, a boolean, a null, the three set types, a list and a nested map), since
//! every one of them is written in the same frame. The companion elements are the same
//! minimal put throughout, so what differs between the rows is the element being read back.
//!
//! The boundary of the same rule is here too: AT the cap the transaction is applied, so the
//! rendering is about the bound and not about the actions.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every action names.
const TABLE: &str = "TransactQuoted";

/// Its key attribute.
const KEY: &str = "pk";

/// The cap the transactional lists declare.
const CAP: usize = 100;

/// The fields of the attribute-value object this service's own request validator renders a
/// value with, in the order it states them — written out here rather than read off the crate
/// under test, so the RENDERING is what these rows are held to.
const CORAL_FIELDS: [&str; 16] = [
    "s", "n", "b", "iNT", "dECIMAL", "dOUBLE", "sS", "nS", "bS", "iNTSET", "dECIMALSET",
    "dOUBLESET", "m", "l", "nULL", "bOOL",
];

/// One value in that frame: the field it carries filled from the request, every other field
/// rendered unset.
fn quoted_value(field: &str, value: &str) -> String {
    let fields: Vec<String> = CORAL_FIELDS
        .iter()
        .map(|declared| {
            if *declared == field {
                format!("{declared}={value}")
            } else {
                format!("{declared}=null")
            }
        })
        .collect();
    format!("AttributeValue({})", fields.join(", "))
}

/// A map in that rendering: `{<name>=<rendered value>}`, the entries in NAME order.
fn quoted_map(entries: &[(&str, String)]) -> String {
    let mut ordered: Vec<(&str, String)> = entries.to_vec();
    ordered.sort_by_key(|(name, _)| *name);
    let rendered: Vec<String> = ordered
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("{{{}}}", rendered.join(", "))
}

/// The fields every quoted action ends with when the request filled none of them: the two
/// substitution tables, what it asks for on a failed condition, and the workload-profile
/// field of the service's own request objects, which this model declares no member for.
const UNFILLED_TRAILING: &str = "expressionAttributeNames=null, expressionAttributeValues=null, \
                                 returnValuesOnConditionCheckFailure=null, \
                                 workloadProfileName=null";

/// The minimal put every row fills its list with: one item of one string attribute.
fn minimal_put(index: usize) -> Value {
    json!({"Put": {"TableName": TABLE, "Item": {KEY: {"S": format!("k{index}")}}}})
}

/// That put as the refusal quotes it.
fn quoted_minimal_put(index: usize) -> String {
    format!(
        "TransactWriteItem(conditionCheck=null, put=Put(item={}, tableName={TABLE}, \
         conditionExpression=null, {UNFILLED_TRAILING}), delete=null, update=null)",
        quoted_map(&[(KEY, quoted_value("s", &format!("k{index}")))])
    )
}

/// The item the `Put` row carries: one attribute per datatype a request can state.
fn every_datatype_item() -> Value {
    json!({
        KEY: {"S": "k0"},
        "tally": {"N": "17"},
        "blob": {"B": "YQ=="},
        "flag": {"BOOL": true},
        "nothing": {"NULL": true},
        "texts": {"SS": ["a", "b"]},
        "numbers": {"NS": ["1", "2"]},
        "blobs": {"BS": ["YQ==", "Yg=="]},
        "list": {"L": [{"S": "x"}, {"N": "3"}]},
        "doc": {"M": {"leaf": {"S": "deep"}}},
    })
}

/// That item as the refusal quotes it.
fn quoted_every_datatype_item() -> String {
    quoted_map(&[
        (KEY, quoted_value("s", "k0")),
        ("tally", quoted_value("n", "17")),
        ("blob", quoted_value("b", "YQ==")),
        ("flag", quoted_value("bOOL", "true")),
        ("nothing", quoted_value("nULL", "true")),
        ("texts", quoted_value("sS", "[a, b]")),
        ("numbers", quoted_value("nS", "[1, 2]")),
        ("blobs", quoted_value("bS", "[YQ==, Yg==]")),
        (
            "list",
            quoted_value(
                "l",
                &format!(
                    "[{}, {}]",
                    quoted_value("s", "x"),
                    quoted_value("n", "3")
                ),
            ),
        ),
        (
            "doc",
            quoted_value("m", &quoted_map(&[("leaf", quoted_value("s", "deep"))])),
        ),
    ])
}

/// The requirement phrase AWS states for the `@length` maximum this list broke.
fn at_most() -> String {
    format!("Member must have length less than or equal to {CAP}")
}

/// The whole message a list of `elements` quoted actions earns.
fn expected_message(elements: &[String]) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at 'TransactItems' failed to satisfy \
         constraint: {}",
        elements.join(", "),
        at_most()
    )
}

#[tokio::test]
async fn a_write_transaction_past_its_cap_quotes_the_actions_it_was_sent() {
    let router = emulator();
    create_active_table(&router, TABLE, &[(KEY, "HASH", "S")]).await;

    // Each row: the case, the first action of the list, and that action as the refusal quotes
    // it. The rest of each list is the same minimal put, so the row is about its own action.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a put of one attribute per datatype",
            json!({"Put": {"TableName": TABLE, "Item": every_datatype_item()}}),
            format!(
                "TransactWriteItem(conditionCheck=null, put=Put(item={}, tableName={TABLE}, \
                 conditionExpression=null, {UNFILLED_TRAILING}), delete=null, update=null)",
                quoted_every_datatype_item()
            ),
        ),
        (
            "a put with a condition and both substitution tables",
            json!({"Put": {
                "TableName": TABLE,
                "Item": {KEY: {"S": "k0"}},
                "ConditionExpression": "#n <> :v",
                "ExpressionAttributeNames": {"#n": "note"},
                "ExpressionAttributeValues": {":v": {"S": "x"}},
                "ReturnValuesOnConditionCheckFailure": "ALL_OLD",
            }}),
            format!(
                "TransactWriteItem(conditionCheck=null, put=Put(item={}, tableName={TABLE}, \
                 conditionExpression=#n <> :v, expressionAttributeNames={{#n=note}}, \
                 expressionAttributeValues={}, \
                 returnValuesOnConditionCheckFailure=ALL_OLD, workloadProfileName=null), \
                 delete=null, update=null)",
                quoted_map(&[(KEY, quoted_value("s", "k0"))]),
                quoted_map(&[(":v", quoted_value("s", "x"))])
            ),
        ),
        (
            "a delete of one key",
            json!({"Delete": {"TableName": TABLE, "Key": {KEY: {"S": "k0"}}}}),
            format!(
                "TransactWriteItem(conditionCheck=null, put=null, delete=Delete(key={}, \
                 tableName={TABLE}, conditionExpression=null, {UNFILLED_TRAILING}), \
                 update=null)",
                quoted_map(&[(KEY, quoted_value("s", "k0"))])
            ),
        ),
        (
            "an update with an expression and its values",
            json!({"Update": {
                "TableName": TABLE,
                "Key": {KEY: {"S": "k0"}},
                "UpdateExpression": "SET tally = :n",
                "ExpressionAttributeValues": {":n": {"N": "4"}},
            }}),
            format!(
                "TransactWriteItem(conditionCheck=null, put=null, delete=null, \
                 update=Update(key={}, updateExpression=SET tally = :n, tableName={TABLE}, \
                 conditionExpression=null, expressionAttributeNames=null, \
                 expressionAttributeValues={}, \
                 returnValuesOnConditionCheckFailure=null, workloadProfileName=null))",
                quoted_map(&[(KEY, quoted_value("s", "k0"))]),
                quoted_map(&[(":n", quoted_value("n", "4"))])
            ),
        ),
        (
            "a condition check on one key",
            json!({"ConditionCheck": {
                "TableName": TABLE,
                "Key": {KEY: {"S": "k0"}},
                "ConditionExpression": "attribute_not_exists(note)",
            }}),
            format!(
                "TransactWriteItem(conditionCheck=ConditionCheck(key={}, tableName={TABLE}, \
                 conditionExpression=attribute_not_exists(note), {UNFILLED_TRAILING}), \
                 put=null, delete=null, update=null)",
                quoted_map(&[(KEY, quoted_value("s", "k0"))])
            ),
        ),
    ];

    for (case, first, quoted_first) in rows {
        // One past the cap: the row's own action, then the minimal put repeated.
        let mut actions: Vec<Value> = vec![first];
        let mut quoted: Vec<String> = vec![quoted_first];
        for index in 1..=CAP {
            actions.push(minimal_put(index));
            quoted.push(quoted_minimal_put(index));
        }
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": actions}),
        )
        .await;
        let expected = expected_message(&quoted);
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?}: expected the message {expected:?}, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_write_transaction_at_its_cap_is_applied_rather_than_quoted() {
    let router = emulator();
    create_active_table(&router, TABLE, &[(KEY, "HASH", "S")]).await;

    // The boundary: a list filled to exactly the cap is served and applied, so the rendering
    // above is about the bound the list broke and not about the actions it carried.
    let actions: Vec<Value> = (0..CAP).map(minimal_put).collect();
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": actions}),
    )
    .await;
    let walked = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        walked["Count"],
        json!(i64::try_from(CAP).unwrap_or_default()),
        "case \"a transaction at the cap\": expected every action applied, got {walked}"
    );

    // And one past the cap writes nothing: the refusal is about the list, so no action of it
    // is applied.
    let mut actions: Vec<Value> = (0..=CAP).map(minimal_put).collect();
    actions.push(minimal_put(CAP + 1));
    let (status, body) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": actions}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a transaction past the cap\": expected the refusal, got {status} with {body}"
    );
    let walked = ok(
        &router,
        "Scan",
        json!({"TableName": TABLE, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        walked["Count"],
        json!(i64::try_from(CAP).unwrap_or_default()),
        "case \"a transaction past the cap\": expected nothing further written, got {walked}"
    );
}
