// SPDX-License-Identifier: CC0-1.0

//! How the BATCHED and TRANSACTIONAL write surfaces refuse a key defect, how an UPDATE's
//! empty secondary-index key is worded, which defects of a transaction are that element's
//! cancellation, and the order several defects of one request are reported in.
//!
//! Every suite here is a TABLE spanning what its rule generalises over rather than one
//! recorded request, and every assertion names its case and the value it actually read.
//!
//! RULE 1 — in the BATCHED write surface, an item whose key does not match the table's
//! `KeySchema` — a key attribute the item omits, or one carrying another type than the
//! table's `AttributeDefinitions` declare for it — is refused with the ONE fixed key-schema
//! sentence, naming no attribute and neither type, whichever of the two defects it was. The
//! `Key`-shaped members of every surface state that same sentence. The TRANSACTIONAL
//! surface states the defect instead: a key attribute of another type is reported as the
//! mismatch between the declared and the supplied type, naming the attribute, inside that
//! element's cancellation reason, while a key attribute the item omits is the fixed
//! sentence. The single-item `PutItem` states its own per-defect wording, which is a
//! different member's rule. The rows span the defect, the key's role in the schema, the
//! supplied type and the surface.
//!
//! WHICH ANSWER states that sentence inside a transaction follows the side that decided
//! it: a key the TABLE's key schema does not admit — a type it does not declare for that
//! attribute, a value past the size limit of the role it gives it, an attribute it declares
//! no key for, or a key element a `Key`-shaped member leaves out — cancels the element
//! (RULE 3), while a member that is not shaped like a key for any schema to be read against
//! — an `Item` that gives no value for a key attribute, a member carrying no attribute at
//! all, an empty key value — is the request's own defect and refuses the call.
//!
//! RULE 2 — an empty `S`/`B` value in a secondary index key is refused either way, and the
//! sentence depends on HOW it arrived: an item a write carried names the index and the
//! index key, while an UPDATE that would leave one empty names the update as what
//! attempted the unsupported change and names neither the index nor the key. The rows span
//! the index's kind, the attribute's role in its key schema, the value type, the spelling
//! of the update (`UpdateExpression` and the legacy `AttributeUpdates`) and the surface.
//!
//! RULE 3 — a defect a transaction decides about one element against the TABLE's own key
//! schema cancels the transaction and is reported as that element's cancellation reason
//! under the `ValidationError` code, with every other element reporting `None`. The rows
//! span the key attribute's role, the position of the offending element, the number of
//! elements and the action that writes it.
//!
//! RULE 4 — a constraint clause about a member of ONE element of a transaction names that
//! member's FULL path: the list, the element's 1-based position in it, the collection step,
//! the action the element states and the leaf member. The rows span the position, the
//! action, and the constraint the name broke.
//!
//! RULE 5 — every fact of the request is settled BEFORE any table is looked up, so a
//! malformed request naming a table that is not there is refused as the malformed request
//! it is; and a violated idempotency token is stated ALONE, masking the other violations of
//! the same request. The rows span the operation, the defect, and whether the table exists.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every suite addresses: a composite primary key, one global secondary index
/// over a string attribute, and one local secondary index over another.
const TABLE: &str = "Catalog";

/// The one key-schema sentence a key that is not the table's primary key is refused with.
const KEY_SCHEMA_MISMATCH: &str = "The provided key element does not match the schema";

/// The clause a cancellation reason opens with when the sentence it carries is about a
/// parameter VALUE ("Code: ValidationError, Message: One or more parameter values were
/// invalid" — the `TransactWriteItems` doc's own list of cancellation reason codes). A
/// reason states the SAME sentence the single-item path states for the same defect, so a
/// sentence that does not open with this clause does not gain it.
const CANCELLATION_CLAUSE: &str = "One or more parameter values were invalid";

/// Create [`TABLE`] and wait out the `CREATING` -> `ACTIVE` transition the clock owes it.
async fn create_indexed_table(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gpk", "AttributeType": "S"},
                {"AttributeName": "gsk", "AttributeType": "B"},
                {"AttributeName": "lsk", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": "gsi1",
                "KeySchema": [
                    {"AttributeName": "gpk", "KeyType": "HASH"},
                    {"AttributeName": "gsk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "LocalSecondaryIndexes": [{
                "IndexName": "lsi1",
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "lsk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    describe_when_active(router, "create the indexed table", TABLE).await;
}

/// Call `operation`, require a 400 `ValidationException`, and answer its message.
async fn refusal(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
         {answered}"
    );
    message(&answered)
}

/// Call the transactional `operation`, require the transaction to be CANCELLED, and
/// answer the `CancellationReasons` codes beside the message of the reason at `at`.
async fn cancellation(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
    at: usize,
) -> (Vec<String>, String) {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "TransactionCanceledException"),
        "case {case:?} ({operation}): expected the transaction cancelled, got {status} with \
         {answered}"
    );
    let reasons = answered["CancellationReasons"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let codes: Vec<String> = reasons
        .iter()
        .map(|reason| reason["Code"].as_str().unwrap_or_default().to_string())
        .collect();
    let stated = reasons
        .get(at)
        .and_then(|reason| reason["Message"].as_str())
        .unwrap_or("<no message>")
        .to_string();
    (codes, stated)
}

/// An item carrying the attributes `members` names, as the JSON a request sends.
fn item(members: &[(&str, Value)]) -> Value {
    let mut built = serde_json::Map::new();
    for (name, value) in members {
        built.insert((*name).to_string(), value.clone());
    }
    Value::Object(built)
}

// ===========================================================================
// RULE 1. A key defect of a batched element states the one key-schema sentence;
//         a transactional element states the type mismatch
// ===========================================================================

/// The detail a transactional element's cancellation reason states for a key attribute of
/// another type than the schema declares: the attribute, its declared type and the supplied
/// one.
fn type_mismatch_detail(attribute: &str, declared: &str, supplied: &str) -> String {
    format!("Type mismatch for key {attribute} expected: {declared} actual: {supplied}")
}

#[tokio::test]
async fn a_multi_item_key_defect_states_its_surfaces_sentence() {
    let router = emulator();
    create_indexed_table(&router).await;

    // Each row: the case, the item the element carries, the sentence the defect earns on the
    // BATCHED surface, whether a TRANSACTION cancels for it (the table's key schema decided
    // it) or refuses the request (the element is not shaped like a key), and — for a key
    // attribute of another TYPE, which is the schema's own defect — the attribute and the two
    // types the transactional surface names in that element's cancellation reason. They span
    // the defect (a key attribute omitted, one of another type, one whose value is empty),
    // the key's ROLE in the schema (partition and sort), the supplied type (`N`, `B`, a set,
    // a map) and an item that breaks both halves at once.
    let empty_key_value = "One or more parameter values are not valid. The AttributeValue for a \
                           key attribute cannot contain an empty string value. Key: pk";
    let rows: Vec<(&str, Value, &str, bool, Option<String>)> = vec![
        (
            "an item omitting the sort key",
            item(&[("pk", json!({"S": "k"}))]),
            KEY_SCHEMA_MISMATCH,
            false,
            None,
        ),
        (
            "an item omitting the partition key",
            item(&[("sk", json!({"S": "s"}))]),
            KEY_SCHEMA_MISMATCH,
            false,
            None,
        ),
        (
            "an item omitting both key attributes",
            item(&[("note", json!({"S": "plain"}))]),
            KEY_SCHEMA_MISMATCH,
            false,
            None,
        ),
        (
            // An empty key VALUE is its own documented refusal, which names the key it was
            // supplied for rather than the schema.
            "an item whose only key attribute carries an empty value",
            item(&[("pk", json!({"S": ""})), ("sk", json!({"S": "s"}))]),
            empty_key_value,
            false,
            None,
        ),
        (
            "a partition key supplied as a number",
            item(&[("pk", json!({"N": "7"})), ("sk", json!({"S": "s"}))]),
            KEY_SCHEMA_MISMATCH,
            true,
            Some(type_mismatch_detail("pk", "S", "N")),
        ),
        (
            "a sort key supplied as a number",
            item(&[("pk", json!({"S": "k"})), ("sk", json!({"N": "7"}))]),
            KEY_SCHEMA_MISMATCH,
            true,
            Some(type_mismatch_detail("sk", "S", "N")),
        ),
        (
            "a partition key supplied as binary",
            item(&[("pk", json!({"B": "AQI="})), ("sk", json!({"S": "s"}))]),
            KEY_SCHEMA_MISMATCH,
            true,
            Some(type_mismatch_detail("pk", "S", "B")),
        ),
        (
            "a sort key supplied as a string set",
            item(&[("pk", json!({"S": "k"})), ("sk", json!({"SS": ["s"]}))]),
            KEY_SCHEMA_MISMATCH,
            true,
            Some(type_mismatch_detail("sk", "S", "SS")),
        ),
        (
            // Both halves at once: the schema is walked in its own order, so the
            // partition key's type is the defect reported.
            "a partition key supplied as a map, the sort key omitted",
            item(&[("pk", json!({"M": {"inner": {"S": "s"}}}))]),
            KEY_SCHEMA_MISMATCH,
            true,
            Some(type_mismatch_detail("pk", "S", "M")),
        ),
    ];

    for (case, carried, stated_defect, cancels, type_mismatch) in rows {
        // The batched surface refuses the whole request with that sentence, whichever of
        // the two defects it was.
        let stated = refusal(
            &router,
            case,
            "BatchWriteItem",
            json!({"RequestItems": {TABLE: [{"PutRequest": {"Item": carried.clone()}}]}}),
        )
        .await;
        let expected_batch = stated_defect.to_string();
        assert_eq!(
            stated, expected_batch,
            "case {case:?} (BatchWriteItem): expected {expected_batch:?}, got {stated:?}"
        );

        let transaction = json!({"TransactItems": [
            {"Put": {"TableName": TABLE, "Item": carried.clone()}},
        ]});
        if !cancels {
            // The element is not shaped like a key, which is a fact of the REQUEST: the
            // call is refused and nothing is cancelled.
            let stated = refusal(&router, case, "TransactWriteItems", transaction).await;
            assert_eq!(
                stated, expected_batch,
                "case {case:?} (TransactWriteItems): expected {expected_batch:?}, got {stated:?}"
            );
            continue;
        }
        // The table's own key schema decided it, so it is that element's cancellation
        // reason — and for a key attribute of another TYPE that reason states the two
        // types behind the documented reason clause rather than the fixed sentence the
        // batched surface answers.
        let (codes, reason) = cancellation(&router, case, "TransactWriteItems", transaction, 0).await;
        assert_eq!(
            codes,
            vec!["ValidationError".to_string()],
            "case {case:?} (TransactWriteItems): expected the element named ValidationError, got \
             {codes:?}"
        );
        let detail = type_mismatch
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: a cancelling row names the two types"));
        let expected = format!("{CANCELLATION_CLAUSE}: {detail}");
        assert_eq!(
            reason, expected,
            "case {case:?} (TransactWriteItems): expected {expected:?}, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn a_key_member_of_every_surface_states_the_same_sentence() {
    let router = emulator();
    create_indexed_table(&router).await;

    // A `Key` IS the primary key, so every way it differs from the schema is the same
    // sentence — on the single-item surfaces and inside the batched one. The rows span the
    // operation, the defect and the member the key arrived in. The TRANSACTIONAL surfaces'
    // `Key` members are not here: every way a non-empty `Key` differs from the table's key
    // schema is decided against that table, so the element cancels with that same sentence
    // (`a_transactional_key_the_schema_does_not_admit_cancels_the_element`).
    let bad_key = json!({"pk": {"S": "k"}, "sk": {"N": "7"}});
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a wrong-typed sort key on GetItem",
            "GetItem",
            json!({"TableName": TABLE, "Key": bad_key.clone()}),
        ),
        (
            "a wrong-typed sort key on UpdateItem",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": bad_key.clone(),
                   "UpdateExpression": "SET note = :v",
                   "ExpressionAttributeValues": {":v": {"S": "x"}}}),
        ),
        (
            "a wrong-typed sort key on DeleteItem",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": bad_key.clone()}),
        ),
        (
            "a key naming an attribute the schema does not declare",
            "GetItem",
            json!({"TableName": TABLE,
                   "Key": {"pk": {"S": "k"}, "sk": {"S": "s"}, "extra": {"S": "x"}}}),
        ),
        (
            "a key omitting the sort key",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "k"}}}),
        ),
        (
            "a batched delete's key",
            "BatchWriteItem",
            json!({"RequestItems": {TABLE: [{"DeleteRequest": {"Key": bad_key.clone()}}]}}),
        ),
        (
            "a batched get's key",
            "BatchGetItem",
            json!({"RequestItems": {TABLE: {"Keys": [bad_key.clone()]}}}),
        ),
        (
            // A `Key` member carrying NO attribute at all states no key for any schema to
            // be read against, so even inside a transaction it is the request's own defect.
            "a transactional write's key carrying no attribute at all",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {"TableName": TABLE, "Key": {}}}]}),
        ),
        (
            "a transactional get's key carrying no attribute at all",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {"TableName": TABLE, "Key": {}}}]}),
        ),
    ];

    for (case, operation, request) in rows {
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, KEY_SCHEMA_MISMATCH,
            "case {case:?} ({operation}): expected {KEY_SCHEMA_MISMATCH:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_transactional_key_the_schema_does_not_admit_cancels_the_element() {
    let router = emulator();
    create_indexed_table(&router).await;

    // The `Key`-shaped members of a transactional WRITE and GET, each carrying a key the
    // TABLE's key schema does not admit: a sort key of a type it does not declare, a
    // partition key one byte past the size limit of the role it plays in that schema, a key
    // element the member leaves out, and an attribute that schema declares no key for.
    // Deciding any of them needs the table, so each is the element's own cancellation under
    // the documented `ValidationError` code rather than a refusal of the request. The rows
    // span the action the key arrived in, the defect, and which half of the schema refused
    // it.
    let bad_key = json!({"pk": {"S": "k"}, "sk": {"N": "7"}});
    // "The maximum length of any partition key value is 2048 bytes" ("Service, Account,
    // and Table Quotas"), so this is one byte past it.
    let oversize_key = json!({"pk": {"S": "x".repeat(2049)}, "sk": {"S": "s"}});
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a transactional delete's wrong-typed sort key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {"TableName": TABLE, "Key": bad_key.clone()}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional delete's key omitting the sort key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {
                "TableName": TABLE, "Key": {"pk": {"S": "k"}}}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional get's key omitting the sort key",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {
                "TableName": TABLE, "Key": {"pk": {"S": "k"}}}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional get's key naming an attribute the schema declares no key for",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {
                "TableName": TABLE,
                "Key": {"pk": {"S": "k"}, "sk": {"S": "s"}, "extra": {"S": "x"}}}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional condition check's wrong-typed sort key",
            "TransactWriteItems",
            json!({"TransactItems": [{"ConditionCheck": {
                "TableName": TABLE, "Key": bad_key.clone(),
                "ConditionExpression": "attribute_exists(pk)"}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional update's wrong-typed sort key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": {
                "TableName": TABLE, "Key": bad_key.clone(),
                "UpdateExpression": "SET note = :v",
                "ExpressionAttributeValues": {":v": {"S": "x"}}}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional delete's oversize partition key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Delete": {
                "TableName": TABLE, "Key": oversize_key.clone()}}]}),
            // The size sentence is itself a refusal of a parameter value, so it carries
            // the clause the documented reason opens with.
            "One or more parameter values were invalid: Size of hashkey has exceeded the \
             maximum size limit of2048 bytes",
        ),
        (
            // The same two keys read out of a transactional GET, which is the other
            // transactional surface: one defect, one answer.
            "a transactional get's wrong-typed sort key",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {"TableName": TABLE, "Key": bad_key.clone()}}]}),
            KEY_SCHEMA_MISMATCH,
        ),
        (
            "a transactional get's oversize partition key",
            "TransactGetItems",
            json!({"TransactItems": [{"Get": {
                "TableName": TABLE, "Key": oversize_key.clone()}}]}),
            "One or more parameter values were invalid: Size of hashkey has exceeded the \
             maximum size limit of2048 bytes",
        ),
    ];

    for (case, operation, request, detail) in rows {
        let (codes, reason) = cancellation(&router, case, operation, request, 0).await;
        assert_eq!(
            codes,
            vec!["ValidationError".to_string()],
            "case {case:?}: expected the element named ValidationError, got {codes:?}"
        );
        assert_eq!(
            reason, detail,
            "case {case:?}: expected {detail:?}, got {reason:?}"
        );
    }
}

#[tokio::test]
async fn the_single_item_put_keeps_its_own_per_defect_wording() {
    let router = emulator();
    create_indexed_table(&router).await;

    // `PutItem`'s `Item` is the whole item rather than a key, and its refusals name the
    // defect: the two types for a key of another type than the schema declares. The rows
    // span the key's role and the supplied type.
    let typed: Vec<(&str, Value, &str, &str, &str)> = vec![
        (
            "a partition key supplied as a number",
            item(&[("pk", json!({"N": "7"})), ("sk", json!({"S": "s"}))]),
            "pk",
            "S",
            "N",
        ),
        (
            "a sort key supplied as a number",
            item(&[("pk", json!({"S": "k"})), ("sk", json!({"N": "7"}))]),
            "sk",
            "S",
            "N",
        ),
        (
            "a sort key supplied as a string set",
            item(&[("pk", json!({"S": "k"})), ("sk", json!({"SS": ["s"]}))]),
            "sk",
            "S",
            "SS",
        ),
    ];

    for (case, carried, attribute, declared, supplied) in typed {
        let expected = format!(
            "One or more parameter values were invalid: Type mismatch for key {attribute} \
             expected: {declared} actual: {supplied}"
        );
        let stated = refusal(
            &router,
            case,
            "PutItem",
            json!({"TableName": TABLE, "Item": carried}),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?} (PutItem): expected {expected:?}, got {stated:?}"
        );
    }

    // An `Item` that omits a key attribute is refused too, and NOT with the key-schema
    // sentence the batched and transactional surfaces state: that member's wording is
    // its own.
    for (case, carried) in [
        ("an item omitting the sort key", item(&[("pk", json!({"S": "k"}))])),
        ("an item omitting the partition key", item(&[("sk", json!({"S": "s"}))])),
    ] {
        let stated = refusal(
            &router,
            case,
            "PutItem",
            json!({"TableName": TABLE, "Item": carried}),
        )
        .await;
        assert_ne!(
            stated, KEY_SCHEMA_MISMATCH,
            "case {case:?} (PutItem): expected the item member's own wording, got {stated:?}"
        );
    }
}

// ===========================================================================
// RULE 2. An empty index key names the index, or the update that emptied it
// ===========================================================================

/// The sentence an item a write CARRIED earns, which names the index and the index key.
fn carried_empty_index_key(kind: &str, index: &str, attribute: &str) -> String {
    format!(
        "One or more parameter values are not valid. A value specified for a secondary index \
         key is not supported. The AttributeValue for a key attribute cannot contain an empty \
         {kind} value. IndexName: {index}, IndexKey: {attribute}"
    )
}

/// The sentence an UPDATE earns, which names neither.
fn updated_empty_index_key(kind: &str) -> String {
    format!(
        "One or more parameter values are not valid. The update expression attempted to \
         update a secondary index key to a value that is not supported. The AttributeValue \
         for a key attribute cannot contain an empty {kind} value."
    )
}

#[tokio::test]
async fn an_update_that_empties_an_index_key_names_the_update_and_not_the_index() {
    let router = emulator();
    create_indexed_table(&router).await;

    // Each row: the case, the operation, the request, and the value type the sentence
    // names. They span the index's kind (global and local), the attribute's role in its
    // key schema (hash and range), the type that can be empty (`S` and `B`), the
    // spelling of the update (`UpdateExpression`, the legacy `AttributeUpdates`) and the
    // surface the update arrived through.
    let key = json!({"pk": {"S": "u1"}, "sk": {"S": "s"}});
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an update emptying a global index's hash key",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key.clone(),
                   "UpdateExpression": "SET gpk = :e",
                   "ExpressionAttributeValues": {":e": {"S": ""}}}),
            "string",
        ),
        (
            "an update emptying a global index's binary range key",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key.clone(),
                   "UpdateExpression": "SET gsk = :e",
                   "ExpressionAttributeValues": {":e": {"B": ""}}}),
            "binary",
        ),
        (
            "an update emptying a local index's range key",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key.clone(),
                   "UpdateExpression": "SET lsk = :e",
                   "ExpressionAttributeValues": {":e": {"S": ""}}}),
            "string",
        ),
        (
            "the same through the legacy AttributeUpdates spelling",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key.clone(),
                   "AttributeUpdates": {"gpk": {"Action": "PUT", "Value": {"S": ""}}}}),
            "string",
        ),
        (
            "a transactional update emptying a global index's hash key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": {
                "TableName": TABLE, "Key": key.clone(),
                "UpdateExpression": "SET gpk = :e",
                "ExpressionAttributeValues": {":e": {"S": ""}}}}]}),
            "string",
        ),
        (
            "a transactional update emptying a local index's range key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": {
                "TableName": TABLE, "Key": key.clone(),
                "UpdateExpression": "SET lsk = :e",
                "ExpressionAttributeValues": {":e": {"S": ""}}}}]}),
            "string",
        ),
    ];

    for (case, operation, request, kind) in rows {
        let expected = updated_empty_index_key(kind);
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_item_a_write_carried_keeps_the_index_naming_sentence() {
    let router = emulator();
    create_indexed_table(&router).await;

    // The same rule, reached through an ITEM rather than an update: the sentence names
    // the index whose key schema the attribute belongs to and the attribute's own name.
    // The rows span the surface, the index's kind and the value type.
    let rows: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "a single-item put",
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": {"pk": {"S": "c1"}, "sk": {"S": "s"}, "gpk": {"S": ""}}}),
            "string",
            "gsi1",
            "gpk",
        ),
        (
            "a batched put",
            "BatchWriteItem",
            json!({"RequestItems": {TABLE: [{"PutRequest": {"Item":
                   {"pk": {"S": "c2"}, "sk": {"S": "s"}, "gsk": {"B": ""}}}}]}}),
            "binary",
            "gsi1",
            "gsk",
        ),
        (
            "a transactional put",
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {"TableName": TABLE, "Item":
                   {"pk": {"S": "c3"}, "sk": {"S": "s"}, "lsk": {"S": ""}}}}]}),
            "string",
            "lsi1",
            "lsk",
        ),
    ];

    for (case, operation, request, kind, index, attribute) in rows {
        let expected = carried_empty_index_key(kind, index, attribute);
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {stated:?}"
        );
    }
}

// ===========================================================================
// RULE 3. An update that moves a key attribute is that element's cancellation
// ===========================================================================

#[tokio::test]
async fn a_transactional_update_of_a_key_attribute_cancels_the_transaction() {
    let router = emulator();
    create_indexed_table(&router).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "t1"}, "sk": {"S": "s"}}}),
    )
    .await;

    let key = json!({"pk": {"S": "t1"}, "sk": {"S": "s"}});
    let other = json!({"Put": {"TableName": TABLE,
                               "Item": {"pk": {"S": "t9"}, "sk": {"S": "s"}}}});
    // A second element nothing is wrong with, addressing its OWN item: "transaction
    // requests cannot include multiple operations on one item", so an element that repeated
    // the one above would be that refusal rather than this rule.
    let another = json!({"Put": {"TableName": TABLE,
                                 "Item": {"pk": {"S": "t8"}, "sk": {"S": "s"}}}});
    let moves_sort = json!({"Update": {
        "TableName": TABLE, "Key": key.clone(),
        "UpdateExpression": "SET sk = :v",
        "ExpressionAttributeValues": {":v": {"S": "moved"}}}});
    let moves_partition = json!({"Update": {
        "TableName": TABLE, "Key": key.clone(),
        "UpdateExpression": "SET pk = :v",
        "ExpressionAttributeValues": {":v": {"S": "moved"}}}});
    let removes_sort = json!({"Update": {
        "TableName": TABLE, "Key": key.clone(),
        "UpdateExpression": "REMOVE sk"}});

    // Each row: the case, the elements, the position of the offending one, the attribute
    // the message names, and the codes every element reports. They span the key
    // attribute's role, the spelling of the update, the offending element's position and
    // the number of elements.
    let rows: Vec<(&str, Value, usize, &str, Vec<&str>)> = vec![
        (
            "a sole element moving the sort key",
            json!([moves_sort.clone()]),
            0,
            "sk",
            vec!["ValidationError"],
        ),
        (
            "a sole element moving the partition key",
            json!([moves_partition.clone()]),
            0,
            "pk",
            vec!["ValidationError"],
        ),
        (
            "a sole element removing the sort key",
            json!([removes_sort.clone()]),
            0,
            "sk",
            vec!["ValidationError"],
        ),
        (
            "the second of two elements",
            json!([other.clone(), moves_sort.clone()]),
            1,
            "sk",
            vec!["None", "ValidationError"],
        ),
        (
            "the middle of three elements",
            json!([other.clone(), moves_partition.clone(), another.clone()]),
            1,
            "pk",
            vec!["None", "ValidationError", "None"],
        ),
    ];

    for (case, elements, at, attribute, codes) in rows {
        let (stated_codes, reason) = cancellation(
            &router,
            case,
            "TransactWriteItems",
            json!({"TransactItems": elements.clone()}),
            at,
        )
        .await;
        let expected_codes: Vec<String> = codes.iter().map(|code| (*code).to_string()).collect();
        assert_eq!(
            stated_codes, expected_codes,
            "case {case:?}: expected the reasons {expected_codes:?}, got {stated_codes:?}"
        );
        let expected = format!(
            "{CANCELLATION_CLAUSE}: Cannot update attribute {attribute}. This attribute is \
             part of the key"
        );
        assert_eq!(
            reason, expected,
            "case {case:?}: expected the reason {expected:?}, got {reason:?}"
        );
    }

    // Nothing the cancelled transactions carried landed, and the item they addressed is
    // untouched.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"pk": {"S": "t1"}, "sk": {"S": "s"}}),
        "case the addressed item: a cancelled transaction changes nothing, got {read}"
    );
    let other_read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "t9"}, "sk": {"S": "s"}},
               "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        other_read.get("Item"),
        None,
        "case the other element's item: a cancelled transaction writes nothing, got \
         {other_read}"
    );

    // The single-item update refuses the same move as the request-level defect it is
    // there: one element of a transaction is the thing that was wrong, and a single
    // update has none.
    let stated = refusal(
        &router,
        "the same move through UpdateItem",
        "UpdateItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "t1"}, "sk": {"S": "s"}},
               "UpdateExpression": "SET sk = :v",
               "ExpressionAttributeValues": {":v": {"S": "moved"}}}),
    )
    .await;
    assert_eq!(
        stated,
        "One or more parameter values were invalid: Cannot update attribute sk. This attribute \
         is part of the key",
        "case the same move through UpdateItem: got {stated:?}"
    );
}

// ===========================================================================
// RULE 4. An element's clause names the element's full member path
// ===========================================================================

#[tokio::test]
async fn a_transactional_elements_clause_names_its_full_member_path() {
    let router = emulator();
    create_indexed_table(&router).await;

    let good_item = json!({"pk": {"S": "p1"}, "sk": {"S": "s"}});
    let good_key = json!({"pk": {"S": "p1"}, "sk": {"S": "s"}});
    let ok_element = json!({"Put": {"TableName": TABLE, "Item": good_item.clone()}});
    let long_name = "n".repeat(256);

    // Each row: the case, the elements, and the path and clause the refusal must state.
    // They span the element's 1-based position, the action its member names, and the
    // constraint the name broke (the pattern, the 3-character minimum, the
    // 255-character maximum).
    let rows: Vec<(&str, Value, String, String)> = vec![
        (
            "a put in the first position, outside the name pattern",
            json!([{"Put": {"TableName": "bad name", "Item": good_item.clone()}}]),
            "transactItems.1.member.put.tableName".to_string(),
            "Value 'bad name' at 'transactItems.1.member.put.tableName' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            "an update in the second position, below the minimum length",
            json!([
                ok_element.clone(),
                {"Update": {"TableName": "ab", "Key": good_key.clone(),
                            "UpdateExpression": "SET note = :v",
                            "ExpressionAttributeValues": {":v": {"S": "x"}}}},
            ]),
            "transactItems.2.member.update.tableName".to_string(),
            "Value 'ab' at 'transactItems.2.member.update.tableName' failed to satisfy \
             constraint: Member must have length greater than or equal to 3"
                .to_string(),
        ),
        (
            "a delete in the third position, past the maximum length",
            json!([
                ok_element.clone(),
                ok_element.clone(),
                {"Delete": {"TableName": long_name.clone(), "Key": good_key.clone()}},
            ]),
            "transactItems.3.member.delete.tableName".to_string(),
            format!(
                "Value '{long_name}' at 'transactItems.3.member.delete.tableName' failed to \
                 satisfy constraint: Member must have length less than or equal to 255"
            ),
        ),
        (
            "a condition check in the second position, outside the pattern",
            json!([
                ok_element.clone(),
                {"ConditionCheck": {"TableName": "bad name", "Key": good_key.clone(),
                                    "ConditionExpression": "attribute_exists(pk)"}},
            ]),
            "transactItems.2.member.conditionCheck.tableName".to_string(),
            "Value 'bad name' at 'transactItems.2.member.conditionCheck.tableName' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: \
             [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
    ];

    for (case, elements, path, clause) in rows {
        let expected = format!("1 validation error detected: {clause}");
        let stated = refusal(
            &router,
            case,
            "TransactWriteItems",
            json!({"TransactItems": elements}),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the clause at {path:?}, got {stated:?}"
        );
    }

    // The request-level member of an operation that carries one keeps the request-level
    // path, so the paths above are the ELEMENT's and not a new spelling of that member.
    let stated = refusal(
        &router,
        "the same name on a single-item write",
        "PutItem",
        json!({"TableName": "bad name", "Item": good_item}),
    )
    .await;
    assert_eq!(
        stated,
        "1 validation error detected: Value 'bad name' at 'tableName' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+",
        "case the same name on a single-item write: got {stated:?}"
    );
}

// ===========================================================================
// RULE 5. Request shape first, and a violated idempotency token alone
// ===========================================================================

#[tokio::test]
async fn request_shape_is_settled_before_the_table_is_looked_up() {
    let router = emulator();
    create_indexed_table(&router).await;
    const MISSING: &str = "NeverCreated";

    // Each row: the case, the operation, the request members beside `TableName`, and the
    // whole message the refusal states. The rows span the operation, the member the
    // defect sits in and the defect itself. (Whether a legacy clause carries the
    // `<N> validation error(s) detected: ` envelope is the existing rule for the member
    // it arrived in, which this suite reads back rather than changes.)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an Expected comparison with no value to compare",
            "PutItem",
            json!({"Item": {"pk": {"S": "k"}, "sk": {"S": "s"}},
                   "Expected": {"a": {"ComparisonOperator": "EQ"}}}),
            "1 validation error detected: One or more parameter values were invalid: Value or \
             AttributeValueList must be used with ComparisonOperator: EQ for Attribute: a",
        ),
        (
            "an Expected entry expecting existence with no value",
            "UpdateItem",
            json!({"Key": {"pk": {"S": "k"}, "sk": {"S": "s"}},
                   "Expected": {"a": {"Exists": true}}}),
            "1 validation error detected: One or more parameter values were invalid: Value \
             must be provided when Exists is true for Attribute: a",
        ),
        (
            "an Expected comparison given too few operands",
            "DeleteItem",
            json!({"Key": {"pk": {"S": "k"}, "sk": {"S": "s"}},
                   "Expected": {"a": {"ComparisonOperator": "BETWEEN",
                                      "AttributeValueList": [{"N": "1"}]}}}),
            "One or more parameter values were invalid: Invalid number of argument(s) for the \
             BETWEEN ComparisonOperator",
        ),
        (
            "an Expected comparison the service does not serve",
            "PutItem",
            json!({"Item": {"pk": {"S": "k"}, "sk": {"S": "s"}},
                   "Expected": {"a": {"ComparisonOperator": "LIKE",
                                      "AttributeValueList": [{"S": "x"}]}}}),
            "Value 'LIKE' at \
             'expected.a.member.comparisonOperator' failed to satisfy constraint: Member must \
             satisfy enum value set: [IN, NULL, BETWEEN, LT, NOT_CONTAINS, EQ, GT, NOT_NULL, \
             NE, LE, BEGINS_WITH, GE, CONTAINS]",
        ),
    ];

    for (case, operation, members, expected) in rows {
        // Against a table that was never created, and against the one that is there:
        // the defect is the REQUEST's, so the same sentence either way.
        for table in [MISSING, TABLE] {
            let mut request = members.clone();
            request["TableName"] = json!(table);
            let stated = refusal(&router, case, operation, request).await;
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}, table {table:?}): expected {expected:?}, got \
                 {stated:?}"
            );
        }
    }

    // A request nothing is wrong with still reports the missing table, so the rows above
    // report their defect because the request was malformed and not because the lookup
    // stopped happening.
    let (status, answered) = call(
        &router,
        "PutItem",
        json!({"TableName": MISSING, "Item": {"pk": {"S": "k"}, "sk": {"S": "s"}}}),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            message(&answered).as_str()
        ),
        (400, "ResourceNotFoundException", "Requested resource not found"),
        "case a well-formed write against a missing table: got {status} with {answered}"
    );
}

#[tokio::test]
async fn an_elements_defect_is_reported_whichever_table_an_earlier_element_names() {
    let router = emulator();
    create_indexed_table(&router).await;

    let good_item = json!({"pk": {"S": "p1"}, "sk": {"S": "s"}});
    let huge_item = json!({"pk": {"S": "p1"}, "sk": {"S": "s"},
                           "blob": {"S": "x".repeat(500 * 1024)}});
    // The FIRST element of every row addresses a table that is not there; the second
    // carries a defect of the request itself. Every fact of the request is settled
    // first, so the second element's defect is what the transaction is refused with.
    // The rows span the member the defect sits in: the element's table name, the item
    // it writes, the flag it states its condition report through, and the grammar of
    // the update it applies.
    let missing = json!({"Put": {"TableName": "NeverCreated", "Item": good_item.clone()}});
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a name that is not a table name",
            json!({"Put": {"TableName": "bad name", "Item": good_item.clone()}}),
            "1 validation error detected: Value 'bad name' at \
             'transactItems.2.member.put.tableName' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
                .to_string(),
        ),
        (
            "an item past the documented item size",
            json!({"Put": {"TableName": TABLE, "Item": huge_item}}),
            "Item size has exceeded the maximum allowed size".to_string(),
        ),
        (
            "a condition report the action does not accept",
            json!({"Put": {"TableName": TABLE, "Item": good_item.clone(),
                           "ReturnValuesOnConditionCheckFailure": "BOGUS"}}),
            "1 validation error detected: Value 'BOGUS' at \
             'transactItems.2.member.put.returnValuesOnConditionCheckFailure' failed to satisfy \
             constraint: Member must satisfy enum value set: [ALL_OLD, NONE]"
                .to_string(),
        ),
        (
            "a substitution the element's expressions never use",
            json!({"Update": {"TableName": TABLE, "Key": good_item.clone(),
                              "UpdateExpression": "SET note = :v",
                              "ExpressionAttributeValues": {":v": {"S": "x"}, ":spare": {"S": "y"}}}}),
            "Value provided in ExpressionAttributeValues unused in expressions: keys: {:spare}"
                .to_string(),
        ),
    ];

    for (case, element, expected) in rows {
        let stated = refusal(
            &router,
            case,
            "TransactWriteItems",
            json!({"TransactItems": [missing.clone(), element]}),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // With nothing wrong with the request itself, the missing table is reported.
    let (status, answered) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [
            {"Put": {"TableName": "NeverCreated", "Item": good_item.clone()}},
            {"Put": {"TableName": TABLE, "Item": good_item}},
        ]}),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            message(&answered).as_str()
        ),
        (400, "ResourceNotFoundException", "Requested resource not found"),
        "case a well-formed transaction naming a missing table: got {status} with {answered}"
    );
}

#[tokio::test]
async fn a_violated_idempotency_token_is_stated_alone() {
    let router = emulator();
    create_indexed_table(&router).await;

    let element = json!({"Put": {"TableName": TABLE,
                                 "Item": {"pk": {"S": "i1"}, "sk": {"S": "s"}}}});
    let over_long = "x".repeat(37);
    let far_over = "x".repeat(100);
    let max_length = "Value at 'ClientRequestToken' failed to satisfy constraint: Member must \
                      have length less than or equal to 36";

    // Each row: the case, the request, and the whole message. They span the token's
    // length (one past the bound and far past it), what else the request carries (a
    // member outside its enumerated set, a second such member, an element whose name is
    // not a table name) and the number of violations the request would otherwise report.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a token one character past the bound, alone",
            json!({"TransactItems": [element.clone()], "ClientRequestToken": over_long.clone()}),
            format!("1 validation error detected: {max_length}"),
        ),
        (
            "the same token beside a value outside an enumerated set",
            json!({"TransactItems": [element.clone()],
                   "ClientRequestToken": over_long.clone(),
                   "ReturnConsumedCapacity": "BOGUS"}),
            format!("1 validation error detected: {max_length}"),
        ),
        (
            "a far longer token beside two other defects",
            json!({"TransactItems": [element.clone()],
                   "ClientRequestToken": far_over.clone(),
                   "ReturnConsumedCapacity": "BOGUS",
                   "ReturnItemCollectionMetrics": "BOGUS"}),
            format!("1 validation error detected: {max_length}"),
        ),
    ];

    for (case, request, expected) in rows {
        let stated = refusal(&router, case, "TransactWriteItems", request).await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The violation the token masked is reported as itself when the token is within its
    // bound, so the rows above state one error because the token masks the rest and not
    // because the other member goes unchecked.
    let stated = refusal(
        &router,
        "the masked violation on its own",
        "TransactWriteItems",
        json!({"TransactItems": [element.clone()],
               "ClientRequestToken": "x".repeat(36),
               "ReturnConsumedCapacity": "BOGUS"}),
    )
    .await;
    assert_eq!(
        stated,
        "1 validation error detected: Value 'BOGUS' at 'returnConsumedCapacity' failed to \
         satisfy constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]",
        "case the masked violation on its own: got {stated:?}"
    );

    // And a token AT the bound is accepted, so the refusals above are the bound rather
    // than an off-by-one.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [element], "ClientRequestToken": "y".repeat(36)}),
    )
    .await;
}

/// A plain table beside the indexed one, so the suites above are not the only shape the
/// batched surfaces are driven over: the same key-schema sentence holds for a table whose
/// primary key is a partition key alone.
#[tokio::test]
async fn the_key_schema_sentence_holds_for_a_single_attribute_key() {
    let router = emulator();
    create_active_table(&router, "Simple", &[("id", "HASH", "N")]).await;

    // The rows span the defect against a one-attribute schema: the key attribute
    // omitted, supplied as another type, and an item carrying only another attribute.
    for (case, carried) in [
        ("the key attribute omitted", json!({"other": {"S": "x"}})),
        (
            "the key attribute supplied as a string",
            json!({"id": {"S": "7"}}),
        ),
        (
            "the key attribute supplied as a boolean",
            json!({"id": {"BOOL": true}}),
        ),
    ] {
        let stated = refusal(
            &router,
            case,
            "BatchWriteItem",
            json!({"RequestItems": {"Simple": [{"PutRequest": {"Item": carried}}]}}),
        )
        .await;
        assert_eq!(
            stated, KEY_SCHEMA_MISMATCH,
            "case {case:?} (BatchWriteItem): expected {KEY_SCHEMA_MISMATCH:?}, got {stated:?}"
        );
    }
}
