// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, end to end over the wire.
//!
//! Three rules are under test, each over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. a constraint violation on an input member is answered as a 400
//!    `ValidationException` whose message is the `<N> validation error(s)
//!    detected: ` envelope over one clause per violated member, each clause
//!    naming the member by its dotted input path and the constraint it failed;
//! 2. a clause states only facts of the REQUEST — the member path, the
//!    constraint, and the rejected value where AWS echoes one — and never an
//!    emulator-internal figure (a configured ceiling, a byte count, the name of
//!    the operation that refused it);
//! 3. a refusal whose text AWS fixes (the item-size ceiling, the nesting-depth
//!    ceiling, a key that is not the table's primary key, the data-plane
//!    `ResourceNotFoundException`) carries exactly that text with nothing
//!    appended.
//!
//! The rows differ along what each rule generalises over: the member, the
//! operation, the violated constraint, the value's size and type, the depth of
//! the member inside the request, and the published bound (each bound is tested
//! at the bound and one past it). Every published bound and value set named in a
//! row comes from the Smithy model — `smithy_query` `shape` for `IndexName`
//! (length 3..255, pattern `^[a-zA-Z0-9_.-]+$`), `PositiveIntegerObject`
//! (range min 1), `KeyList` / `BatchGetRequestMap` / `TransactGetItemList` /
//! `TransactWriteItemList` (length 1..100) and the `ReturnConsumedCapacity`,
//! `Select` and `ReturnValue` enums — and from the `CreateTable$TableName` doc
//! for the 3..255 `[a-zA-Z0-9_.-]+` table-name form.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4-shaped `Authorization` header: its credential scope routes the
/// request to this service and names the region. Request authentication is off
/// in these tests, so the signature itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/us-east-1/dynamodb/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000";

/// One emulator holding only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one operation and return its HTTP status and parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Send one operation that must succeed.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, &body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` the refusal carries.
fn message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Call `operation`, require a 400 with `expected_code`, and return the message.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    expected_code: &str,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, expected_code),
        "case {case:?} ({operation}): expected 400 {expected_code}, got {status} {body}"
    );
    message(&body)
}

/// Create a table from `(attribute, key type, attribute type)` rows and wait out
/// the `CREATING` -> `ACTIVE` transition the clock owes it.
async fn create_active_table(
    router: &MultiServiceRouter,
    name: &str,
    keys: &[(&str, &str, &str)],
) {
    let key_schema: Vec<Value> = keys
        .iter()
        .map(|(attribute, key_type, _)| json!({"AttributeName": attribute, "KeyType": key_type}))
        .collect();
    let definitions: Vec<Value> = keys
        .iter()
        .map(|(attribute, _, attribute_type)| {
            json!({"AttributeName": attribute, "AttributeType": attribute_type})
        })
        .collect();
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": key_schema,
            "AttributeDefinitions": definitions,
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        described["Table"]["TableStatus"],
        json!("ACTIVE"),
        "case create {name}: the table becomes ACTIVE once the transition is due, got {described}"
    );
}

/// `count` distinct primary keys for the single-key table below.
fn keys(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"Artist": {"S": format!("a{index}")}}))
        .collect()
}

/// `count` `TransactGetItems` entries for that table.
fn transact_gets(table: &str, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({"Get": {"TableName": table, "Key": {"Artist": {"S": format!("a{index}")}}}})
        })
        .collect()
}

/// `count` `TransactWriteItems` entries for that table.
fn transact_puts(table: &str, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({"Put": {"TableName": table, "Item": {"Artist": {"S": format!("a{index}")}}}})
        })
        .collect()
}

/// Nothing an emulator-internal figure could reach a caller through: a
/// configured ceiling, a byte total, or the emulator naming itself.
const INTERNAL_DETAIL: [&str; 6] = [
    "409600",
    "bytes",
    "emulator",
    "32 levels",
    "PutItem",
    "DeleteItem",
];

#[tokio::test]
async fn a_constraint_violation_is_stated_in_the_validation_envelope() {
    let router = emulator();
    create_active_table(&router, "Music", &[("Artist", "HASH", "S")]).await;

    // Each row: the case, the operation, the request, and the WHOLE message the
    // refusal must carry. The rows span the constraint classes (range, length on
    // a string, length on a list, length on a map, pattern, enum value set,
    // absent required member), the member's depth in the request (a top-level
    // member, and a list nested under a map entry the CALLER keyed), and both a
    // bound and one past it.
    let long_name = "a".repeat(256);
    let at_bound_name = "a".repeat(255);
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "Limit at one below its range minimum",
            "Query",
            json!({"TableName": "Music", "Limit": 0, "KeyConditionExpression": "Artist = :a",
                   "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
            "1 validation error detected: Value at 'Limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "Limit far below its range minimum states the same requirement",
            "Query",
            json!({"TableName": "Music", "Limit": -7, "KeyConditionExpression": "Artist = :a",
                   "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
            "1 validation error detected: Value at 'Limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "the same bound on another operation's Limit",
            "Scan",
            json!({"TableName": "Music", "Limit": 0}),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "the same bound on a control-plane listing's Limit",
            "ListBackups",
            json!({"Limit": 0}),
            "1 validation error detected: Value '0' at 'limit' failed to satisfy constraint: \
             Member must have value greater than or equal to 1"
                .to_string(),
        ),
        (
            "a key list one past the 100-key cap, nested under the caller's own map key",
            "BatchGetItem",
            json!({"RequestItems": {"Music": {"Keys": keys(101)}}}),
            "1 validation error detected: Value at 'RequestItems.Music.member.Keys' failed to \
             satisfy constraint: Member must have length less than or equal to 100"
                .to_string(),
        ),
        (
            "an empty key list, one below the same member's minimum",
            "BatchGetItem",
            json!({"RequestItems": {"Music": {"Keys": []}}}),
            "1 validation error detected: Value at 'RequestItems.Music.member.Keys' failed to \
             satisfy constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "an empty RequestItems map, one below the map's own minimum",
            "BatchGetItem",
            json!({"RequestItems": {}}),
            "1 validation error detected: Value at 'RequestItems' failed to satisfy constraint: \
             Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "a transaction one past the 100-item cap",
            "TransactGetItems",
            json!({"TransactItems": transact_gets("Music", 101)}),
            "1 validation error detected: Value at 'TransactItems' failed to satisfy constraint: \
             Member must have length less than or equal to 100"
                .to_string(),
        ),
        (
            "a write transaction one past the same cap",
            "TransactWriteItems",
            json!({"TransactItems": transact_puts("Music", 101)}),
            "1 validation error detected: Value at 'TransactItems' failed to satisfy constraint: \
             Member must have length less than or equal to 100"
                .to_string(),
        ),
        (
            "an IndexName outside its pattern reports the expression unanchored",
            "Query",
            json!({"TableName": "Music", "IndexName": "bad name!",
                   "KeyConditionExpression": "Artist = :a",
                   "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
            "1 validation error detected: Value at 'IndexName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            "an IndexName one below its length minimum",
            "Query",
            json!({"TableName": "Music", "IndexName": "ab",
                   "KeyConditionExpression": "Artist = :a",
                   "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
            "1 validation error detected: Value at 'IndexName' failed to satisfy constraint: \
             Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "a value outside an enum's set echoes the value the caller sent",
            "GetItem",
            json!({"TableName": "Music", "Key": {"Artist": {"S": "Acme"}},
                   "ReturnConsumedCapacity": "NOPE"}),
            "1 validation error detected: Value 'NOPE' at 'returnConsumedCapacity' failed to \
             satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]"
                .to_string(),
        ),
        (
            "another member's enum set, on another operation",
            "Query",
            json!({"TableName": "Music", "Select": "EVERYTHING",
                   "KeyConditionExpression": "Artist = :a",
                   "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
            "1 validation error detected: Value 'EVERYTHING' at 'select' failed to satisfy \
             constraint: Member must satisfy enum value set: [ALL_ATTRIBUTES, \
             ALL_PROJECTED_ATTRIBUTES, SPECIFIC_ATTRIBUTES, COUNT]"
                .to_string(),
        ),
        (
            "a required member the request does not carry",
            "GetItem",
            json!({"TableName": "Music"}),
            "1 validation error detected: Value null at 'key' failed to satisfy constraint: \
             Member must not be null"
                .to_string(),
        ),
        (
            "a table name one below the documented 3-character minimum",
            "GetItem",
            json!({"TableName": "ab", "Key": {"Artist": {"S": "Acme"}}}),
            "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy constraint: \
             Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "the same bound on the control plane",
            "DescribeTable",
            json!({"TableName": "ab"}),
            "1 validation error detected: Value 'ab' at 'tableName' failed to satisfy constraint: \
             Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "a table name one past the documented 255-character maximum",
            "GetItem",
            json!({"TableName": long_name.clone(), "Key": {"Artist": {"S": "Acme"}}}),
            format!(
                "1 validation error detected: Value '{long_name}' at 'tableName' failed to \
                 satisfy constraint: Member must have length less than or equal to 255"
            ),
        ),
        (
            "a table name outside the documented pattern",
            "PutItem",
            json!({"TableName": "bad name!", "Item": {"Artist": {"S": "Acme"}}}),
            "1 validation error detected: Value 'bad name!' at 'tableName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
    ];

    for (case, operation, request, expected) in cases {
        let stated = refusal(&router, case, operation, &request, "ValidationException").await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // A name AT each published bound is accepted, so the refusals above are the
    // bound and not an off-by-one: the 3-character name resolves to a missing
    // table (a data-plane not-found), the 255-character one likewise.
    for name in ["abc", at_bound_name.as_str()] {
        let stated = refusal(
            &router,
            "a table name at a published bound",
            "GetItem",
            &json!({"TableName": name, "Key": {"Artist": {"S": "Acme"}}}),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated, "Requested resource not found",
            "case a table name at a published bound ({} chars): expected the not-found text, \
             got {stated:?}",
            name.len()
        );
    }
}

#[tokio::test]
async fn two_violations_are_counted_and_joined_in_one_envelope() {
    let router = emulator();
    create_active_table(&router, "Music", &[("Artist", "HASH", "S")]).await;

    let stated = refusal(
        &router,
        "a Limit below its minimum and a Select outside its set",
        "Query",
        &json!({"TableName": "Music", "Limit": 0, "Select": "EVERYTHING",
                "KeyConditionExpression": "Artist = :a",
                "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
        "ValidationException",
    )
    .await;

    let limit_clause = "Value at 'Limit' failed to satisfy constraint: Member must have \
                        value greater than or equal to 1";
    let select_clause = "Value 'EVERYTHING' at 'select' failed to satisfy constraint: Member \
                         must satisfy enum value set: [ALL_ATTRIBUTES, ALL_PROJECTED_ATTRIBUTES, \
                         SPECIFIC_ATTRIBUTES, COUNT]";
    assert!(
        stated.starts_with("2 validation errors detected: "),
        "case two violations: expected the plural envelope counting both, got {stated:?}"
    );
    assert!(
        stated.contains(limit_clause) && stated.contains(select_clause),
        "case two violations: expected both clauses, got {stated:?}"
    );
    assert!(
        stated.contains("; "),
        "case two violations: expected the clauses joined by '; ', got {stated:?}"
    );
}

#[tokio::test]
async fn a_fixed_text_refusal_carries_nothing_appended() {
    let router = emulator();
    create_active_table(&router, "Music", &[("Artist", "HASH", "S")]).await;

    // Rows span the fixed-text refusals and the sizes/values that reach them: an
    // item past the 400 KB item ceiling (twice over, and just past it), an
    // attribute past the 32-level nesting ceiling (at 33 and at 60 levels), each
    // `ReturnValue` an item write does not accept, and each way a `Key` can fail
    // to be the table's primary key.
    let huge = "x".repeat(500 * 1024);
    let bigger = "x".repeat(900 * 1024);
    let mut deep_33 = json!({"S": "bottom"});
    for _ in 0..33 {
        deep_33 = json!({"L": [deep_33]});
    }
    let mut deep_60 = json!({"M": {"inner": {"S": "bottom"}}});
    for _ in 0..60 {
        deep_60 = json!({"M": {"inner": deep_60}});
    }
    let item_size_text = "Item size has exceeded the maximum allowed size";
    let nesting_text = "1 validation error detected: Nesting Levels have exceeded supported \
                        limits: Attributes in the item have nested levels beyond supported limit";
    let return_values_text = "1 validation error detected: ReturnValues can only be ALL_OLD or \
                              NONE";
    let key_text = "The provided key element does not match the schema";

    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an item past the 400 KB ceiling",
            "PutItem",
            json!({"TableName": "Music",
                   "Item": {"Artist": {"S": "Acme"}, "Body": {"S": huge}}}),
            item_size_text,
        ),
        (
            "an item twice past the same ceiling states the same text",
            "PutItem",
            json!({"TableName": "Music",
                   "Item": {"Artist": {"S": "Acme"}, "Body": {"S": bigger}}}),
            item_size_text,
        ),
        (
            "a list nested one level past the 32-level ceiling",
            "PutItem",
            json!({"TableName": "Music",
                   "Item": {"Artist": {"S": "Acme"}, "Nested": deep_33}}),
            nesting_text,
        ),
        (
            "a map nested far past the same ceiling",
            "PutItem",
            json!({"TableName": "Music",
                   "Item": {"Artist": {"S": "Acme"}, "Nested": deep_60}}),
            nesting_text,
        ),
        (
            "ReturnValues=ALL_NEW on a delete",
            "DeleteItem",
            json!({"TableName": "Music", "Key": {"Artist": {"S": "Acme"}},
                   "ReturnValues": "ALL_NEW"}),
            return_values_text,
        ),
        (
            "ReturnValues=UPDATED_OLD on a delete",
            "DeleteItem",
            json!({"TableName": "Music", "Key": {"Artist": {"S": "Acme"}},
                   "ReturnValues": "UPDATED_OLD"}),
            return_values_text,
        ),
        (
            "ReturnValues=UPDATED_NEW on a put",
            "PutItem",
            json!({"TableName": "Music", "Item": {"Artist": {"S": "Acme"}},
                   "ReturnValues": "UPDATED_NEW"}),
            return_values_text,
        ),
        (
            "ReturnValues=ALL_NEW on a put",
            "PutItem",
            json!({"TableName": "Music", "Item": {"Artist": {"S": "Acme"}},
                   "ReturnValues": "ALL_NEW"}),
            return_values_text,
        ),
        (
            "a Key naming an attribute the schema does not declare",
            "GetItem",
            json!({"TableName": "Music",
                   "Key": {"Artist": {"S": "Acme"}, "Spare": {"S": "x"}}}),
            key_text,
        ),
        (
            "a Key omitting the partition key",
            "GetItem",
            json!({"TableName": "Music", "Key": {}}),
            key_text,
        ),
        (
            "a Key whose element is of another type",
            "DeleteItem",
            json!({"TableName": "Music", "Key": {"Artist": {"N": "1"}}}),
            key_text,
        ),
        (
            "a Key whose element is a set rather than a scalar",
            "GetItem",
            json!({"TableName": "Music", "Key": {"Artist": {"SS": ["a"]}}}),
            key_text,
        ),
        (
            "a key attribute given an empty string",
            "PutItem",
            json!({"TableName": "Music", "Item": {"Artist": {"S": ""}}}),
            "One or more parameter values are not valid. The AttributeValue for a key attribute \
             cannot contain an empty string value. Key: Artist",
        ),
    ];

    for (case, operation, request, expected) in cases {
        let stated = refusal(&router, case, operation, &request, "ValidationException").await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_data_plane_states_one_fixed_not_found_and_the_control_plane_names_the_table() {
    let router = emulator();
    create_active_table(&router, "Music", &[("Artist", "HASH", "S")]).await;

    // Every item operation, against a table that is absent and against one that
    // exists but is not yet serving items: the same fixed sentence, naming
    // neither the table nor its status.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Pending",
            "KeySchema": [{"AttributeName": "Artist", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Artist", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;

    let data_plane: Vec<(&str, &str, Value)> = vec![
        (
            "a read of an absent table",
            "GetItem",
            json!({"TableName": "Absent", "Key": {"Artist": {"S": "Acme"}}}),
        ),
        (
            "a write into an absent table",
            "PutItem",
            json!({"TableName": "Absent", "Item": {"Artist": {"S": "Acme"}}}),
        ),
        (
            "a delete from an absent table",
            "DeleteItem",
            json!({"TableName": "Absent", "Key": {"Artist": {"S": "Acme"}}}),
        ),
        (
            "a query of an absent table",
            "Query",
            json!({"TableName": "Absent", "KeyConditionExpression": "Artist = :a",
                   "ExpressionAttributeValues": {":a": {"S": "Acme"}}}),
        ),
        (
            "a read of a table that is still CREATING",
            "GetItem",
            json!({"TableName": "Pending", "Key": {"Artist": {"S": "Acme"}}}),
        ),
        (
            "a write into a table that is still CREATING",
            "PutItem",
            json!({"TableName": "Pending", "Item": {"Artist": {"S": "Acme"}}}),
        ),
    ];

    for (case, operation, request) in data_plane {
        let stated = refusal(
            &router,
            case,
            operation,
            &request,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated, "Requested resource not found",
            "case {case:?} ({operation}): expected the fixed data-plane text, got {stated:?}"
        );
        for leak in INTERNAL_DETAIL {
            assert!(
                !stated.contains(leak),
                "case {case:?} ({operation}): the message must not carry {leak:?}, got {stated:?}"
            );
        }
        assert!(
            !stated.contains("Absent") && !stated.contains("Pending"),
            "case {case:?} ({operation}): the data-plane message names no resource, got {stated:?}"
        );
    }

    // The control plane addressed the table BY NAME and reports it back, so its
    // not-found keeps the resource detail.
    for (case, operation, request) in [
        (
            "a describe of an absent table",
            "DescribeTable",
            json!({"TableName": "Absent"}),
        ),
        (
            "a delete of an absent table",
            "DeleteTable",
            json!({"TableName": "Absent"}),
        ),
    ] {
        let stated = refusal(
            &router,
            case,
            operation,
            &request,
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            stated, "Requested resource not found: Table: Absent not found",
            "case {case:?} ({operation}): expected the control-plane text naming the table, \
             got {stated:?}"
        );
    }
}

#[tokio::test]
async fn no_refusal_message_carries_an_emulator_internal_figure() {
    let router = emulator();
    create_active_table(&router, "Music", &[("Artist", "HASH", "S")]).await;

    // One row per refusal family, each checked against the SAME list of things a
    // message must never contain: a configured ceiling, a byte total, the
    // emulator naming itself, or the operation that refused the request.
    let huge = "x".repeat(500 * 1024);
    let families: Vec<(&str, &str, Value)> = vec![
        (
            "the item-size ceiling",
            "PutItem",
            json!({"TableName": "Music",
                   "Item": {"Artist": {"S": "Acme"}, "Body": {"S": huge}}}),
        ),
        (
            "the nesting ceiling",
            "PutItem",
            json!({"TableName": "Music",
                   "Item": {"Artist": {"S": "Acme"},
                            "Nested": {"L": [{"L": [{"L": [{"S": "x"}]}]}]}}}),
        ),
        (
            "a ReturnValues an item write does not accept",
            "DeleteItem",
            json!({"TableName": "Music", "Key": {"Artist": {"S": "Acme"}},
                   "ReturnValues": "ALL_NEW"}),
        ),
        (
            "a collection past its cap",
            "TransactGetItems",
            json!({"TransactItems": transact_gets("Music", 101)}),
        ),
        (
            "a member below its range minimum",
            "Scan",
            json!({"TableName": "Music", "Limit": 0}),
        ),
    ];

    for (case, operation, request) in families {
        let (status, body) = call(&router, operation, &request).await;
        if status == 200 {
            // The shallow nesting row is inside every ceiling and is stored:
            // nothing to word.
            continue;
        }
        let stated = message(&body);
        for leak in INTERNAL_DETAIL {
            assert!(
                !stated.contains(leak),
                "case {case:?} ({operation}): the message must not carry {leak:?}, got {stated:?}"
            );
        }
    }
}
