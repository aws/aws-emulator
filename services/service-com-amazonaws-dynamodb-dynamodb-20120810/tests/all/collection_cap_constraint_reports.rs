// SPDX-License-Identifier: CC0-1.0

//! How this service reports a COLLECTION input member that is outside the
//! `@length` bound the model declares for it — the batched and transactional
//! caps a client retries against.
//!
//! One rule, over a TABLE of inputs rather than one recorded request: a
//! collection input member whose element or entry count falls outside its
//! modelled `@length` bound is refused with HTTP 400 `ValidationException`
//! whose message is the `<N> validation error(s) detected: ` envelope over one
//! clause per violated member, and that clause states the member's dotted input
//! path as the operation that was served names it, that the member failed a
//! constraint, and the BOUND it broke — or, where the bound broken is the one
//! declared on a MAP's VALUES rather than on the member itself, the value
//! shape's WHOLE declared set inside AWS's `Map value must satisfy constraint:
//! [ … ]` sentence, stated at the map member.
//! It states nothing the emulator computed
//! about the request: not the element count, not a byte total, not the operation
//! that refused it. The members a WRITE states its actions in are quoted BESIDE
//! that — the object rendering this service states a refused structure value in,
//! built from the request itself — which is what the batched write's own map and
//! the transactional write's action list carry here. At the bound the request is
//! not refused as a constraint violation at all.
//!
//! The rows differ along what the rule generalises over: the operation
//! (transactional read, transactional write, batched read, batched write), the
//! collection's kind (a list of structures, a list of maps, a map of lists), the
//! bound that was broken (the `@length` maximum and the `@length` minimum), the
//! published bound's value (100 and 25), the member's depth in the request (a
//! top-level member and a list nested under a map entry the CALLER keyed), the
//! shape of the elements (a `Get`, a `Put`, a `Delete`), and how far past the
//! bound the request went (one past it, and half again as many) — each bound
//! tested AT the bound and one past it.
//!
//! Every bound named in a row comes from the Smithy model, through
//! `smithy_query(query="shape", shape="…")`: `TransactGetItemList` and
//! `TransactWriteItemList` are `@length{min:1,max:100}`, `KeyList` and
//! `BatchGetRequestMap` are `@length{min:1,max:100}`, and
//! `BatchWriteItemRequestMap` and `WriteRequests` are `@length{min:1,max:25}`.
//!
//! WHICH SPELLING the member is named by is a property of the OPERATION, not of the
//! member: `BatchGetItem` names its `RequestItems` map by the wire spelling and
//! `BatchWriteItem` names the map of the same name by the lowerCamel path the request
//! validator builds, so both are pinned here side by side (see "Which form a constraint
//! clause takes" in `src/service.rs`).
//!
//! The transactional `TransactItems` lists are pinned at BOTH ends of their `@length` bound
//! here, because the two ends are one rule: `tests/all/transact_items_cap_refusal.rs`
//! drives the maximum over the shapes of the actions those lists carry.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message};
use crate::table_items::create_active_table;

/// The table every row addresses: one hash key, so a `Key` is one attribute.
const TABLE: &str = "Music";

/// The requirement phrase AWS states for a `@length` maximum of `max`.
fn at_most(max: usize) -> String {
    format!("Member must have length less than or equal to {max}")
}

/// The requirement phrase AWS states for a `@length` minimum of `min`.
fn at_least(min: usize) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// The whole message a single violated member earns: the envelope over one
/// clause naming `path` and `requirement`, with no value echoed.
fn report(path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value at '{path}' failed to satisfy constraint: \
         {requirement}"
    )
}

/// The same message for a member whose refusal QUOTES the collection the request carried.
fn quoting_report(path: &str, quoted: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{quoted}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement a clause states at a MAP member for a bound its VALUES broke: the value
/// shape's whole declared set, maximum first, inside the sentence AWS nests it in.
fn map_value_set(max: usize, min: usize) -> String {
    format!(
        "Map value must satisfy constraint: [{}, {}]",
        at_most(max),
        at_least(min)
    )
}

/// The fields of the attribute-value object this service's own request validator renders a
/// value with, in the order it states them — written out here rather than read off the crate
/// under test, so the RENDERING is what these rows are held to.
const CORAL_FIELDS: [&str; 16] = [
    "s", "n", "b", "iNT", "dECIMAL", "dOUBLE", "sS", "nS", "bS", "iNTSET", "dECIMALSET",
    "dOUBLESET", "m", "l", "nULL", "bOOL",
];

/// One value in that frame: the field it carries filled from the request, every other field
/// rendered unset.
fn attribute_value(field: &str, value: &str) -> String {
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

/// How a refused `TransactWriteItems` list of `count` puts is quoted: one element per action,
/// every member of the action structures spelled out and every member the request left out
/// rendered unset.
fn quoted_transact_puts(count: usize) -> String {
    let elements: Vec<String> = (0..count)
        .map(|index| {
            format!(
                "TransactWriteItem(conditionCheck=null, put=Put(item={{Artist={}}}, \
                 tableName={TABLE}, conditionExpression=null, expressionAttributeNames=null, \
                 expressionAttributeValues=null, \
                 returnValuesOnConditionCheckFailure=null, workloadProfileName=null), \
                 delete=null, update=null)",
                attribute_value("s", &format!("a{index}"))
            )
        })
        .collect();
    format!("[{}]", elements.join(", "))
}

/// `count` `TransactGetItems` entries, each a `Get` of a distinct primary key.
fn transact_gets(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"Get": {"TableName": TABLE, "Key": key(index)}}))
        .collect()
}

/// `count` `TransactWriteItems` entries, each a `Put` of a distinct item.
fn transact_puts(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"Put": {"TableName": TABLE, "Item": key(index)}}))
        .collect()
}

/// `count` distinct primary keys for that table.
fn keys(count: usize) -> Vec<Value> {
    (0..count).map(key).collect()
}

/// `count` `BatchWriteItem` write requests for that table.
fn write_requests(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"PutRequest": {"Item": key(index)}}))
        .collect()
}

/// A `BatchWriteItem` request map keyed by `count` distinct table names, each
/// carrying one write request.
fn write_request_tables(count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| {
                (
                    format!("{TABLE}{index:02}"),
                    json!([{"PutRequest": {"Item": key(index)}}]),
                )
            })
            .collect(),
    )
}

/// A request map naming the one table above: `{ "<table>": <value> }`. Built
/// here rather than inside `json!` so the table name stays the one constant.
fn for_table(value: Value) -> Value {
    Value::Object([(TABLE.to_string(), value)].into_iter().collect())
}

/// The primary key of item `index`.
fn key(index: usize) -> Value {
    json!({"Artist": {"S": format!("a{index}")}})
}

/// Nothing the emulator computed about the request may reach a caller through a
/// constraint clause: an element or entry count, a byte total, or the name of
/// the operation that refused it.
const INTERNAL_DETAIL: [&str; 8] = [
    "101 items",
    "150 items",
    "26 items",
    "26 entries",
    " bytes",
    "emulator",
    "TransactGetItems",
    "BatchWriteItem",
];

#[tokio::test]
async fn a_collection_outside_its_modelled_length_bound_is_reported_against_that_bound() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("Artist", "HASH", "S")]).await;

    // Each row: the case, the operation, the request, and the WHOLE message the
    // refusal must carry.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "a transactional read one past the 100-action cap",
            "TransactGetItems",
            json!({"TransactItems": transact_gets(101)}),
            report("TransactItems", &at_most(100)),
        ),
        (
            "a transactional write one past the same cap",
            "TransactWriteItems",
            json!({"TransactItems": transact_puts(101)}),
            quoting_report(
                "TransactItems",
                &quoted_transact_puts(101),
                &at_most(100),
            ),
        ),
        (
            "an empty transactional read, one below the same member's minimum",
            "TransactGetItems",
            json!({"TransactItems": []}),
            report("TransactItems", &at_least(1)),
        ),
        (
            "an empty transactional write, one below the same member's minimum",
            "TransactWriteItems",
            json!({"TransactItems": []}),
            report("TransactItems", &at_least(1)),
        ),
        (
            "a batched read's key list one past the 100-key cap, under the caller's own key",
            "BatchGetItem",
            json!({"RequestItems": for_table(json!({"Keys": keys(101)}))}),
            report(&format!("RequestItems.{TABLE}.member.Keys"), &at_most(100)),
        ),
        (
            "an empty key list, one below that nested member's minimum",
            "BatchGetItem",
            json!({"RequestItems": for_table(json!({"Keys": []}))}),
            report(&format!("RequestItems.{TABLE}.member.Keys"), &at_least(1)),
        ),
        (
            "a batched read naming no table at all, one below the map's own minimum",
            "BatchGetItem",
            json!({"RequestItems": {}}),
            report("RequestItems", &at_least(1)),
        ),
        (
            "a batched write one past the 25-table cap",
            "BatchWriteItem",
            json!({"RequestItems": write_request_tables(26)}),
            report("requestItems", &at_most(25)),
        ),
        (
            "a batched write naming no table at all, one below the same map's minimum",
            "BatchWriteItem",
            json!({"RequestItems": {}}),
            report("requestItems", &at_least(1)),
        ),
        (
            // The same member reached by giving the one table it names an empty list: that
            // is a bound on the map's VALUES rather than on its entries, so the clause
            // quotes the map and states the value shape's whole set.
            "a batched write whose only table was given no request, below the value bound",
            "BatchWriteItem",
            json!({"RequestItems": for_table(json!([]))}),
            quoting_report(
                "requestItems",
                &format!("{{{TABLE}=[]}}"),
                &map_value_set(25, 1),
            ),
        ),
    ];

    for (case, operation, request, expected) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        let (code, stated) = (error_code(&body), message(&body));
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} {code} \
             with {stated:?}"
        );
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
        for leaked in INTERNAL_DETAIL {
            assert!(
                !stated.contains(leaked),
                "case {case:?} ({operation}): the clause must not state {leaked:?}, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_collection_at_its_modelled_bound_is_not_refused_as_a_constraint_violation() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("Artist", "HASH", "S")]).await;

    // The same members, filled to exactly the published bound: whatever each
    // operation answers (these four are not implemented yet, so they answer the
    // scaffold's 501), none of them may be refused as a constraint violation —
    // which is what makes the rows above the bound and not an off-by-one.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a transactional read filled to the 100-item cap",
            "TransactGetItems",
            json!({"TransactItems": transact_gets(100)}),
        ),
        (
            "a transactional read of one item, at the same member's minimum",
            "TransactGetItems",
            json!({"TransactItems": transact_gets(1)}),
        ),
        (
            "a transactional write filled to the 100-item cap",
            "TransactWriteItems",
            json!({"TransactItems": transact_puts(100)}),
        ),
        (
            "a batched read's key list filled to the 100-key cap",
            "BatchGetItem",
            json!({"RequestItems": for_table(json!({"Keys": keys(100)}))}),
        ),
        (
            "a batched write filled to the 25-table cap",
            "BatchWriteItem",
            json!({"RequestItems": write_request_tables(25)}),
        ),
        (
            "a batched write of 25 requests for one table, at the list's own cap",
            "BatchWriteItem",
            json!({"RequestItems": for_table(json!(write_requests(25)))}),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call(&router, operation, request).await;
        let (code, stated) = (error_code(&body), message(&body));
        assert!(
            !(code == "ValidationException" && stated.contains("failed to satisfy constraint")),
            "case {case:?} ({operation}): a collection at its published bound is not a \
             constraint violation, got {status} {code} with {stated:?}"
        );
    }
}
