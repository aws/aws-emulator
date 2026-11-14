// SPDX-License-Identifier: CC0-1.0

//! Two request-validation rules, each over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. an item whose attribute is part of a SECONDARY-INDEX key schema must carry
//!    the `AttributeType` the table's `AttributeDefinitions` declare for that
//!    attribute — "If you specify any attributes that are part of an index key,
//!    then the data types for those attributes must match those of the schema in
//!    the table's attribute definition" (the `PutItem$Item` doc) — so a write
//!    that supplies another type is refused with HTTP 400 `ValidationException`,
//!    the item never lands, and the message is built from the request: the index
//!    key attribute, the declared type, the type supplied and the index whose key
//!    schema the attribute belongs to; and an `S`/`B` index key must be non-empty
//!    ("Attribute values of type String and Binary must have a length greater than
//!    zero if the attribute is used as a key attribute for a table or index" — the
//!    same doc), refused by the sentence that names the index and the index key.
//!    The rules hold on every write path, and an attribute that is the TABLE's own
//!    key keeps the refusal that names it as the table key it is. Which ENVELOPE
//!    carries the sentence follows what the refusal was decided against: the
//!    transactional write cancels its whole request over a fault of the item as the
//!    TABLE's schema sees it (the type rule), naming the refused element's
//!    `ValidationError` reason, and answers a fault of the VALUE the request carried
//!    (the emptiness rule) directly, like every other write path;
//! 2. a numeric input member outside the `@range` its model declares is refused by
//!    a clause whose FORM follows the OPERATION it was refused on: the item-request
//!    form names the member by the spelling the model gives it on the wire and
//!    echoes no value, while the request validator's form names it by the lowerCamel
//!    input path that validator builds and echoes the NUMBER the request carried.
//!
//! The rows differ along what each rule generalises over. For rule 1: the declared
//! type (`S`, `N`, `B`), the type supplied (a scalar of another type, a set, a
//! boolean, a list, a map), the index's kind (global and local), the attribute's
//! position in the index key schema (hash and range), and the operation
//! (`PutItem`, `UpdateItem`, `BatchWriteItem`, `TransactWriteItems`). For rule 2:
//! the clause form (the item-request one and the request validator's), the
//! operation, the member, which bound was broken (minimum and maximum), the
//! published bound's value, and how far past it the value went — each bound tested
//! AT the bound and one past it.
//!
//! Every bound and type named in a row comes from the Smithy model.
//! `smithy_query(query="operation", operation="…")` reports `ListBackups$Limit`
//! `@range{min:1,max:100}`, `ListExports$MaxResults` and `ListImports$PageSize`
//! `@range{min:1,max:25}`, `ListTables$Limit` `@range{min:1,max:100}`,
//! `Query$Limit` and `Scan$Limit` `@range{min:1}`, `Scan$TotalSegments`
//! `@range{min:1,max:1000000}`, `Scan$Segment` `@range{min:0,max:999999}` and
//! `ListContributorInsights$MaxResults` `@range{max:100}`; the `AttributeType`
//! value set (`S`, `N`, `B`) and the `AttributeValue` member names echoed as the
//! supplied type come from
//! `smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// The table every row of rule 1 addresses: a composite primary key, four global
/// secondary indexes and one local secondary index, so one attribute belongs to
/// exactly one index key schema and the index a refusal names is unambiguous.
const TABLE: &str = "catalog";

/// Create [`TABLE`] and wait out the `CREATING` -> `ACTIVE` transition the clock
/// owes it.
async fn create_indexed_table(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gsiS", "AttributeType": "S"},
                {"AttributeName": "gsiN", "AttributeType": "N"},
                {"AttributeName": "gsiB", "AttributeType": "B"},
                {"AttributeName": "gsiP", "AttributeType": "S"},
                {"AttributeName": "gsiR", "AttributeType": "N"},
                {"AttributeName": "lsiN", "AttributeType": "N"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "by-str",
                    "KeySchema": [{"AttributeName": "gsiS", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "by-num",
                    "KeySchema": [{"AttributeName": "gsiN", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "by-bin",
                    "KeySchema": [{"AttributeName": "gsiB", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "by-pair",
                    "KeySchema": [
                        {"AttributeName": "gsiP", "KeyType": "HASH"},
                        {"AttributeName": "gsiR", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "local-num",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "lsiN", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, "DescribeTable", json!({"TableName": TABLE})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case create {TABLE}: the table becomes ACTIVE once the transition is due, got {described}"
    );
}

/// Call `operation`, require a 400 `ValidationException`, and return the message.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
    );
    message(&body)
}

/// One row of rule 1: a write whose index key attribute carries the wrong type,
/// and the four facts the refusal must state about it.
struct MistypedIndexKey {
    case: &'static str,
    operation: &'static str,
    request: Value,
    attribute: &'static str,
    expected: &'static str,
    actual: &'static str,
    index: &'static str,
}

/// Whether the refusal arrives as a CANCELLATION: true only where the fault is
/// one the transactional write decides about the item as the TABLE's schema sees
/// it, which cancels the whole request ("all of which must either succeed or
/// fail") and states the sentence as that element's `CancellationReasons` entry
/// under the `ValidationError` code. A fault of the VALUE the request carried
/// needs no table to decide, so every write path answers it directly.
fn cancels(operation: &str, against_the_tables_schema: bool) -> bool {
    against_the_tables_schema && operation == "TransactWriteItems"
}

/// An item carrying `pk`/`sk` plus `attribute` set to `value`.
fn item_with(key: &str, attribute: &str, value: Value) -> Value {
    json!({"pk": {"S": key}, "sk": {"S": "s"}, attribute: value})
}

/// Drive one write that must be refused and return the sentence it stated: the
/// refused element's cancellation reason when `cancelled`, else the top-level
/// message.
async fn refused_write_sentence(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
    cancelled: bool,
) -> String {
    if !cancelled {
        return refusal(router, case, operation, request).await;
    }
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "TransactionCanceledException"),
        "case {case:?} ({operation}): expected the transaction to be cancelled, got {status} \
         {body}"
    );
    let reasons = body["CancellationReasons"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let codes: Vec<String> = reasons
        .iter()
        .map(|reason| reason["Code"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        codes,
        vec!["ValidationError".to_string()],
        "case {case:?} ({operation}): expected the refused element named ValidationError, got \
         {body}"
    );
    reasons
        .first()
        .and_then(|reason| reason["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string()
}

/// The whole message a mistyped index key earns, built from the request the way
/// AWS builds it.
fn type_mismatch_message(
    attribute: &str,
    expected: &str,
    actual: &str,
    index: &str,
) -> String {
    format!(
        "One or more parameter values were invalid: Type mismatch for Index Key {attribute} \
         Expected: {expected} Actual: {actual} IndexName: {index}"
    )
}

/// The whole message an EMPTY index key earns, built from the request the same way.
fn empty_index_key_message(attribute: &str, kind: &str, index: &str) -> String {
    format!(
        "One or more parameter values are not valid. A value specified for a secondary index \
         key is not supported. The AttributeValue for a key attribute cannot contain an empty \
         {kind} value. IndexName: {index}, IndexKey: {attribute}"
    )
}

/// The whole message an empty index key earns when it is an UPDATE that would leave it
/// empty: the sentence names the update as what attempted the unsupported change and
/// states the rule it broke, naming neither the index nor the key.
fn empty_updated_index_key_message(kind: &str) -> String {
    format!(
        "One or more parameter values are not valid. The update expression attempted to \
         update a secondary index key to a value that is not supported. The AttributeValue \
         for a key attribute cannot contain an empty {kind} value."
    )
}

/// The sentence a refusal states for `detail`.
///
/// A cancellation reason opens with the documented `ValidationError` clause
/// ("Code: ValidationError, Message: One or more parameter values were invalid")
/// and keeps the detail of what the request got wrong behind it; a refusal
/// answered directly states that detail alone.
fn stated_for(cancelled: bool, detail: &str) -> String {
    const CANCELLATION_CLAUSE: &str = "One or more parameter values were invalid";
    if cancelled && !detail.starts_with(CANCELLATION_CLAUSE) {
        return format!("{CANCELLATION_CLAUSE}: {detail}");
    }
    detail.to_string()
}

#[tokio::test]
async fn an_index_key_attribute_of_the_wrong_type_is_refused_on_every_write_path() {
    let router = emulator();
    create_indexed_table(&router).await;

    let rows = vec![
        MistypedIndexKey {
            case: "a global index's hash key declared S, supplied as a number",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k01", "gsiS", json!({"N": "7"}))}),
            attribute: "gsiS",
            expected: "S",
            actual: "N",
            index: "by-str",
        },
        MistypedIndexKey {
            case: "a global index's hash key declared N, supplied as a string",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k02", "gsiN", json!({"S": "7"}))}),
            attribute: "gsiN",
            expected: "N",
            actual: "S",
            index: "by-num",
        },
        MistypedIndexKey {
            case: "a global index's hash key declared B, supplied as a string",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k03", "gsiB", json!({"S": "QQ=="}))}),
            attribute: "gsiB",
            expected: "B",
            actual: "S",
            index: "by-bin",
        },
        MistypedIndexKey {
            case: "a global index's hash key declared S, supplied as a string set",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k04", "gsiS", json!({"SS": ["a", "b"]}))}),
            attribute: "gsiS",
            expected: "S",
            actual: "SS",
            index: "by-str",
        },
        MistypedIndexKey {
            case: "a global index's hash key declared N, supplied as a boolean",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k05", "gsiN", json!({"BOOL": true}))}),
            attribute: "gsiN",
            expected: "N",
            actual: "BOOL",
            index: "by-num",
        },
        MistypedIndexKey {
            case: "a global index's RANGE key declared N, supplied as a string",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k06", "gsiR", json!({"S": "x"}))}),
            attribute: "gsiR",
            expected: "N",
            actual: "S",
            index: "by-pair",
        },
        MistypedIndexKey {
            case: "a global index's hash key declared S, supplied as a list",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k07", "gsiP", json!({"L": [{"S": "a"}]}))}),
            attribute: "gsiP",
            expected: "S",
            actual: "L",
            index: "by-pair",
        },
        MistypedIndexKey {
            case: "a LOCAL index's RANGE key declared N, supplied as a string",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k08", "lsiN", json!({"S": "x"}))}),
            attribute: "lsiN",
            expected: "N",
            actual: "S",
            index: "local-num",
        },
        MistypedIndexKey {
            case: "a LOCAL index's RANGE key declared N, supplied as a map",
            operation: "PutItem",
            request: json!({"TableName": TABLE,
                            "Item": item_with("k09", "lsiN", json!({"M": {"a": {"S": "b"}}}))}),
            attribute: "lsiN",
            expected: "N",
            actual: "M",
            index: "local-num",
        },
        MistypedIndexKey {
            case: "an update that would leave the index key a string",
            operation: "UpdateItem",
            request: json!({"TableName": TABLE,
                            "Key": {"pk": {"S": "k10"}, "sk": {"S": "s"}},
                            "UpdateExpression": "SET gsiN = :v",
                            "ExpressionAttributeValues": {":v": {"S": "nope"}}}),
            attribute: "gsiN",
            expected: "N",
            actual: "S",
            index: "by-num",
        },
        MistypedIndexKey {
            case: "a batched put whose binary index key is a string",
            operation: "BatchWriteItem",
            request: json!({"RequestItems": {TABLE: [
                {"PutRequest": {"Item": item_with("k11", "gsiB", json!({"S": "nope"}))}},
            ]}}),
            attribute: "gsiB",
            expected: "B",
            actual: "S",
            index: "by-bin",
        },
        MistypedIndexKey {
            case: "a transactional put whose local index key is a number set",
            operation: "TransactWriteItems",
            request: json!({"TransactItems": [
                {"Put": {"TableName": TABLE,
                         "Item": item_with("k12", "lsiN", json!({"NS": ["1", "2"]}))}},
            ]}),
            attribute: "lsiN",
            expected: "N",
            actual: "NS",
            index: "local-num",
        },
    ];

    for row in rows {
        let case = row.case;
        let operation = row.operation;
        // The type rule is decided against the TABLE's `AttributeDefinitions`, so
        // the transactional write reports it as the refused element's cancellation.
        let cancelled = cancels(operation, true);
        let expected = stated_for(
            cancelled,
            &type_mismatch_message(row.attribute, row.expected, row.actual, row.index),
        );
        let stated =
            refused_write_sentence(&router, case, operation, row.request, cancelled).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // The refused writes never landed: every key above is still absent, so the
    // table did not accumulate an item an index could not hold.
    for index in 1..=12 {
        let key = format!("k{index:02}");
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": key}, "sk": {"S": "s"}}}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case {key:?}: a refused write must leave no item behind, got {read}"
        );
    }
}

#[tokio::test]
async fn an_index_key_attribute_of_the_declared_type_round_trips() {
    let router = emulator();
    create_indexed_table(&router).await;

    // Rows that must be ACCEPTED, so the refusal above is the type rule and not a
    // blanket refusal of items carrying index keys: every index key at its
    // declared type, an item carrying none of them, and a NON-key attribute whose
    // type no definition constrains.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "every index key at its declared type",
            json!({"pk": {"S": "ok1"}, "sk": {"S": "s"},
                   "gsiS": {"S": "text"}, "gsiN": {"N": "7"}, "gsiB": {"B": "QQ=="},
                   "gsiP": {"S": "pair"}, "gsiR": {"N": "-3.5"}, "lsiN": {"N": "42"}}),
        ),
        (
            "an item carrying no index key at all",
            json!({"pk": {"S": "ok2"}, "sk": {"S": "s"}, "note": {"S": "plain"}}),
        ),
        (
            "a non-key attribute of any type beside a correctly typed index key",
            json!({"pk": {"S": "ok3"}, "sk": {"S": "s"}, "gsiN": {"N": "1"},
                   "note": {"L": [{"BOOL": false}, {"NULL": true}]}}),
        ),
        (
            "an EMPTY non-key attribute, which the key rule does not speak about",
            json!({"pk": {"S": "ok4"}, "sk": {"S": "s"}, "note": {"S": ""},
                   "blob": {"B": ""}}),
        ),
    ];

    for (case, item) in accepted {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": item.clone()}),
        )
        .await;
        let key = json!({"pk": item["pk"].clone(), "sk": item["sk"].clone()});
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": key, "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            read["Item"], item,
            "case {case:?}: expected the item to read back as written, got {read}"
        );
    }
}

#[tokio::test]
async fn an_empty_index_key_attribute_is_refused_on_every_write_path() {
    let router = emulator();
    create_indexed_table(&router).await;

    // Rows differ in the emptiable value type (`S` and `B`), the index's kind
    // (global and local), the attribute's position in the index key schema, the
    // write path, and whether the empty value arrives in an ITEM the request carried
    // or as what an UPDATE would leave behind — which is the one thing the two
    // sentences differ about. The last field says which of the two the row expects.
    let rows: Vec<(&str, &str, Value, &str, &str, &str, bool)> = vec![
        (
            "a global index's hash key as an empty string",
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": item_with("e01", "gsiS", json!({"S": ""}))}),
            "gsiS",
            "string",
            "by-str",
            false,
        ),
        (
            "a global index's hash key as an empty binary value",
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": item_with("e02", "gsiB", json!({"B": ""}))}),
            "gsiB",
            "binary",
            "by-bin",
            false,
        ),
        (
            "a two-element index's hash key as an empty string",
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": item_with("e03", "gsiP", json!({"S": ""}))}),
            "gsiP",
            "string",
            "by-pair",
            false,
        ),
        (
            "an update that would leave a global index's hash key empty",
            "UpdateItem",
            json!({"TableName": TABLE,
                   "Key": {"pk": {"S": "e04"}, "sk": {"S": "s"}},
                   "UpdateExpression": "SET gsiS = :v",
                   "ExpressionAttributeValues": {":v": {"S": ""}}}),
            "gsiS",
            "string",
            "by-str",
            true,
        ),
        (
            "a batched put whose binary index key is empty",
            "BatchWriteItem",
            json!({"RequestItems": {TABLE: [
                {"PutRequest": {"Item": item_with("e05", "gsiB", json!({"B": ""}))}},
            ]}}),
            "gsiB",
            "binary",
            "by-bin",
            false,
        ),
        (
            "a transactional put whose index key is an empty string",
            "TransactWriteItems",
            json!({"TransactItems": [
                {"Put": {"TableName": TABLE,
                         "Item": item_with("e06", "gsiS", json!({"S": ""}))}},
            ]}),
            "gsiS",
            "string",
            "by-str",
            false,
        ),
        (
            "a transactional update that would leave a binary index key empty",
            "TransactWriteItems",
            json!({"TransactItems": [
                {"Update": {"TableName": TABLE,
                            "Key": {"pk": {"S": "e07"}, "sk": {"S": "s"}},
                            "UpdateExpression": "SET gsiB = :v",
                            "ExpressionAttributeValues": {":v": {"B": ""}}}},
            ]}),
            "gsiB",
            "binary",
            "by-bin",
            true,
        ),
    ];

    for (case, operation, request, attribute, kind, index, through_an_update) in rows {
        // The emptiness rule speaks about the value the request carried, so it is
        // answered directly on every write path, the transactional one included.
        let cancelled = cancels(operation, false);
        let detail = if through_an_update {
            empty_updated_index_key_message(kind)
        } else {
            empty_index_key_message(attribute, kind, index)
        };
        let expected = stated_for(cancelled, &detail);
        let stated = refused_write_sentence(&router, case, operation, request, cancelled).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // None of those writes landed.
    for index in 1..=7 {
        let key = format!("e{index:02}");
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": key}, "sk": {"S": "s"}}}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case {key:?}: a refused write must leave no item behind, got {read}"
        );
    }
}

#[tokio::test]
async fn a_table_key_of_the_wrong_type_keeps_the_table_key_refusal() {
    let router = emulator();
    create_indexed_table(&router).await;

    // `pk` is the table's partition key AND the hash key of the local secondary
    // index, so it plays both roles; the refusal names the role the TABLE gave it.
    let stated = refusal(
        &router,
        "the table's own partition key supplied as a number",
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"N": "7"}, "sk": {"S": "s"}}}),
    )
    .await;
    assert_eq!(
        stated,
        "One or more parameter values were invalid: Type mismatch for key pk expected: S \
         actual: N",
        "case \"the table's own partition key supplied as a number\": expected the table-key \
         refusal, got {stated:?}"
    );

    // An empty table key likewise keeps the table-key wording, not the index one,
    // though `pk` is also the local index's hash key.
    let stated = refusal(
        &router,
        "the table's own partition key supplied as an empty string",
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": ""}, "sk": {"S": "s"}}}),
    )
    .await;
    assert_eq!(
        stated,
        "One or more parameter values are not valid. The AttributeValue for a key attribute \
         cannot contain an empty string value. Key: pk",
        "case \"the table's own partition key supplied as an empty string\": expected the \
         table-key refusal, got {stated:?}"
    );
}

/// One row of rule 2: an operation, the member the request sets, the path the
/// clause must name it by, the value sent, and the bound phrase the clause must
/// state. `table` is the `TableName` the operation also requires, and `echoes`
/// is whether the clause echoes the rejected number — which, with the path's
/// spelling, is what the request surface decides.
struct RangeRow {
    case: &'static str,
    operation: &'static str,
    member: &'static str,
    path: &'static str,
    value: i64,
    requirement: &'static str,
    table: Option<&'static str>,
    echoes: bool,
}

/// The requirement phrase AWS states for a `@range` minimum of `min`.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase AWS states for a `@range` maximum of `max`.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The request body a row sends.
fn range_request(row: &RangeRow) -> Value {
    let mut body = json!({row.member: row.value});
    if let Some(table) = row.table {
        body["TableName"] = json!(table);
    }
    body
}

#[tokio::test]
async fn a_numeric_member_outside_its_range_is_refused_in_its_operation_s_form() {
    let router = emulator();
    create_indexed_table(&router).await;

    let rows: Vec<RangeRow> = vec![
        RangeRow {
            case: "a backup listing limit one below its minimum",
            operation: "ListBackups",
            member: "Limit",
            path: "limit",
            value: 0,
            requirement: "",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "the same member far below the same minimum",
            operation: "ListBackups",
            member: "Limit",
            path: "limit",
            value: -41,
            requirement: "",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "the same member one past its maximum",
            operation: "ListBackups",
            member: "Limit",
            path: "limit",
            value: 101,
            requirement: "max 100",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "an export listing page size one past its maximum",
            operation: "ListExports",
            member: "MaxResults",
            path: "maxResults",
            value: 26,
            requirement: "max 25",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "the same member one below its minimum",
            operation: "ListExports",
            member: "MaxResults",
            path: "maxResults",
            value: 0,
            requirement: "",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "an import listing page size one past the same maximum",
            operation: "ListImports",
            member: "PageSize",
            path: "pageSize",
            value: 26,
            requirement: "max 25",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "a table listing limit one past its maximum",
            operation: "ListTables",
            member: "Limit",
            path: "limit",
            value: 101,
            requirement: "max 100",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "a contributor-insights page size one past its maximum",
            operation: "ListContributorInsights",
            member: "MaxResults",
            path: "maxResults",
            value: 101,
            requirement: "max 100",
            table: None,
            echoes: true,
        },
        RangeRow {
            case: "a data-plane read limit one below its minimum",
            operation: "Query",
            member: "Limit",
            path: "Limit",
            value: 0,
            requirement: "",
            table: Some(TABLE),
            echoes: false,
        },
        RangeRow {
            case: "the same bound on another data-plane read, far below it",
            operation: "Scan",
            member: "Limit",
            path: "limit",
            value: -7,
            requirement: "",
            table: Some(TABLE),
            echoes: true,
        },
        RangeRow {
            case: "a parallel-scan segment count one past its maximum",
            operation: "Scan",
            member: "TotalSegments",
            path: "TotalSegments",
            value: 1_000_001,
            requirement: "max 1000000",
            table: Some(TABLE),
            echoes: false,
        },
        RangeRow {
            case: "a parallel-scan segment count one below its minimum",
            operation: "Scan",
            member: "TotalSegments",
            path: "TotalSegments",
            value: 0,
            requirement: "",
            table: Some(TABLE),
            echoes: false,
        },
        RangeRow {
            case: "a parallel-scan segment one past its maximum",
            operation: "Scan",
            member: "Segment",
            path: "Segment",
            value: 1_000_000,
            requirement: "max 999999",
            table: Some(TABLE),
            echoes: false,
        },
        RangeRow {
            case: "a parallel-scan segment one below its zero minimum",
            operation: "Scan",
            member: "Segment",
            path: "Segment",
            value: -1,
            requirement: "min 0",
            table: Some(TABLE),
            echoes: false,
        },
    ];

    for row in &rows {
        let case = row.case;
        // The bound phrase the row names, resolved from the published bound.
        let requirement = match row.requirement {
            "max 100" => at_most(100),
            "max 25" => at_most(25),
            "max 1000000" => at_most(1_000_000),
            "max 999999" => at_most(999_999),
            "min 0" => at_least(0),
            _ => at_least(1),
        };
        let subject = if row.echoes {
            format!("Value '{}'", row.value)
        } else {
            "Value".to_string()
        };
        let expected = format!(
            "1 validation error detected: {subject} at '{}' failed to satisfy constraint: {}",
            row.path, requirement
        );
        let stated = refusal(&router, case, row.operation, range_request(row)).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({}): expected the message {expected:?}, got {stated:?}",
            row.operation
        );
        // A clause that echoes no value echoes NONE of it: the rejected number is
        // nowhere in the message a data-plane refusal states.
        assert!(
            row.echoes || !stated.contains(&row.value.to_string()),
            "case {case:?} ({}): expected no rejected value echoed, got {stated:?}",
            row.operation
        );
    }
}

/// One row of the at-the-bound table: a request whose numeric members all sit ON
/// a published bound, which must therefore not be refused as a constraint
/// violation at all.
struct AtBound {
    case: &'static str,
    operation: &'static str,
    request: Value,
}

#[tokio::test]
async fn a_numeric_member_at_a_published_bound_is_not_a_constraint_violation() {
    let router = emulator();
    create_indexed_table(&router).await;

    let rows: Vec<AtBound> = vec![
        AtBound {
            case: "a backup listing limit at its minimum",
            operation: "ListBackups",
            request: json!({"Limit": 1}),
        },
        AtBound {
            case: "a backup listing limit at its maximum",
            operation: "ListBackups",
            request: json!({"Limit": 100}),
        },
        AtBound {
            case: "an export listing page size at its minimum",
            operation: "ListExports",
            request: json!({"MaxResults": 1}),
        },
        AtBound {
            case: "an export listing page size at its maximum",
            operation: "ListExports",
            request: json!({"MaxResults": 25}),
        },
        AtBound {
            case: "an import listing page size at its maximum",
            operation: "ListImports",
            request: json!({"PageSize": 25}),
        },
        AtBound {
            case: "a table listing limit at its maximum",
            operation: "ListTables",
            request: json!({"Limit": 100}),
        },
        AtBound {
            case: "a contributor-insights page size at its maximum",
            operation: "ListContributorInsights",
            request: json!({"MaxResults": 100}),
        },
        AtBound {
            case: "a data-plane read limit at its minimum",
            operation: "Query",
            request: json!({"TableName": TABLE, "Limit": 1,
                            "KeyConditionExpression": "pk = :p",
                            "ExpressionAttributeValues": {":p": {"S": "ok1"}}}),
        },
        AtBound {
            case: "a parallel scan at both segment bounds' minimum",
            operation: "Scan",
            request: json!({"TableName": TABLE, "Limit": 1,
                            "TotalSegments": 1, "Segment": 0}),
        },
        AtBound {
            case: "a parallel scan at both segment bounds' maximum",
            operation: "Scan",
            request: json!({"TableName": TABLE,
                            "TotalSegments": 1_000_000, "Segment": 999_999}),
        },
    ];

    for row in rows {
        let case = row.case;
        let operation = row.operation;
        let (status, body) = call(&router, operation, row.request).await;
        let stated = message(&body);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {case:?} ({operation}): a value at a published bound is no constraint \
             violation, got {status} {stated:?}"
        );
    }
}
