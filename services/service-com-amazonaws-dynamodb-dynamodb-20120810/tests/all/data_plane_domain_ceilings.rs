// SPDX-License-Identifier: CC0-1.0

//! The documented DOMAINS and CEILINGS the data plane holds a request to, driven end to
//! end over every surface that carries one.
//!
//! The rule this suite pins, in one sentence: a data-plane request whose values fall
//! outside a documented domain (the `N` numeric domain, the set-member domain, an index's
//! vector schema) or past a documented ceiling (400 KB per item, 32 nesting levels, 2048
//! bytes of partition key, 4 MB per transaction, 25 write requests per batch, 1 000 000
//! scan segments, 100 `IN` operands) is refused with HTTP 400 `ValidationException`
//! BEFORE anything is stored — one refusal for the whole call, whichever surface carried
//! the value — while a request ON the published bound is served; and a PartiQL statement
//! is evaluated with DynamoDB's own semantics for a missing attribute, for a removal that
//! addresses nothing, for a projected document path, and for an index that does not
//! project what the statement filters on.
//!
//! Each test states one half of that rule over a TABLE of inputs rather than one request:
//! the rows differ along the surface the value arrives through (a single-item write, a
//! batched write, a transaction, an expression value, a statement parameter), the type of
//! the value, the direction it left its domain in, and how far past the bound it sits —
//! with the published bound itself driven beside every refusal, so no row passes by
//! refusing everything.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same `MultiServiceRouter`
//! the emulator's front doors use, and every wait is a poll for the condition it waits on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok, REGION};

/// The envelope a clause about a value an item-API MEMBER carried is stated inside; a
/// clause about a value an expression, a statement, a batch slot or a transaction slot
/// carried is stated on its own.
const ENVELOPE: &str = "1 validation error detected: ";

/// The clause every refusal of a PARAMETER VALUE in this service opens with.
const INVALID_PARAMETER: &str = "One or more parameter values were invalid";

/// AWS's fixed sentence for a request that CARRIED an item past the documented 400 KB
/// ceiling ("The maximum item size in DynamoDB is 400 KB" — "Service, Account, and Table
/// Quotas" in the Amazon DynamoDB Developer Guide).
const ITEM_TOO_BIG: &str = "Item size has exceeded the maximum allowed size";

/// AWS's fixed sentence for the documented nesting ceiling ("DynamoDB supports nested
/// attributes up to 32 levels deep", same source).
const NESTING: &str = "Nesting Levels have exceeded supported limits: Attributes in the item \
                       have nested levels beyond supported limit";

/// The documented ceilings this suite drives, each read from the Developer Guide's
/// "Service, Account, and Table Quotas" and held by `state` under the same name.
const MAX_ITEM_BYTES: usize = 400 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_PARTITION_KEY_BYTES: usize = 2048;

/// The documented numeric domain: "Number: ... can have up to 38 digits of precision.
/// Exponent range is 10^-130 to 10^125" (same source).
const MAX_SIGNIFICANT_DIGITS: usize = 38;
const MAX_EXPONENT: i32 = 125;
const MIN_EXPONENT: i32 = -130;

/// Drive one call that must be REFUSED as the request's own defect, answering the message
/// it was refused with.
async fn refused(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
         {answered}"
    );
    message(&answered)
}

/// Whether the table holds NO item under the partition key `pk`, read back through
/// `GetItem` — what a refused write must leave behind.
async fn absent(router: &MultiServiceRouter, table: &str, pk: &str) -> bool {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": {"pk": {"S": pk}}}),
    )
    .await;
    read.get("Item").is_none_or(|item| !item.is_object())
}

/// A value nesting `levels` containers deep, alternating list and map so the walk is held
/// to the DEPTH and not to one container kind.
fn nested(levels: usize) -> Value {
    let mut value = json!({"S": "bottom"});
    for level in 0..levels {
        value = if level % 2 == 0 {
            json!({"L": [value]})
        } else {
            json!({"M": {"inner": value}})
        };
    }
    value
}

/// An item under `pk` whose `body` attribute carries `payload` single-byte characters, so
/// the item stores `pk` + its value + `body` + `payload` bytes
/// (`state::item_size` sums each attribute's name and value).
fn sized_item(pk: &str, payload: usize) -> Value {
    json!({"pk": {"S": pk}, "body": {"S": "x".repeat(payload)}})
}

/// The payload that makes [`sized_item`] store exactly `bytes`: the two attribute names
/// and the key's own value are four bytes between them.
fn payload_for(pk: &str, bytes: usize) -> usize {
    bytes - ("pk".len() + pk.len() + "body".len())
}

/// One active table with a single string partition key.
async fn partitioned(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
}

// =============================================================================
// THE DOCUMENTED WRITE CEILINGS
// =============================================================================

/// One ceiling row: the call that crosses a documented ceiling, the whole message it must
/// earn, and the partition key it tried to write (`None` where the key itself is the
/// defect, so there is no legal key to read back).
struct Ceiling {
    case: &'static str,
    operation: &'static str,
    body: Value,
    expected: String,
    probe: Option<&'static str>,
}

const CEILINGS: &str = "DomainCeilings";

#[tokio::test]
async fn a_write_past_a_documented_ceiling_refuses_the_whole_call_and_stores_nothing() {
    let router = emulator();
    partitioned(&router, CEILINGS).await;

    // The rows cross each documented ceiling on each surface that carries an item: a
    // single-item write, a batched write and a transaction. WHICH envelope a surface
    // states its clause in is that surface's own — a clause about a value an item-API
    // member carried is stated inside the counting envelope, one about a value a batch or
    // transaction slot carried on its own — so each row carries the envelope its surface
    // uses and the whole message is asserted.
    let over = MAX_ITEM_BYTES + 1;
    let deep = nested(MAX_DEPTH + 1);
    let deeper = nested(MAX_DEPTH + 28);
    let long_key = "k".repeat(MAX_PARTITION_KEY_BYTES + 1);
    let rows: Vec<Ceiling> = vec![
        Ceiling {
            case: "a single-item write one byte past the item ceiling",
            operation: "PutItem",
            body: json!({
                "TableName": CEILINGS,
                "Item": sized_item("over", payload_for("over", over)),
            }),
            expected: ITEM_TOO_BIG.to_string(),
            probe: Some("over"),
        },
        Ceiling {
            case: "a single-item write far past the item ceiling states the same sentence",
            operation: "PutItem",
            body: json!({
                "TableName": CEILINGS,
                "Item": sized_item("far", payload_for("far", 3 * MAX_ITEM_BYTES)),
            }),
            expected: ITEM_TOO_BIG.to_string(),
            probe: Some("far"),
        },
        Ceiling {
            case: "the same item inside a batched write",
            operation: "BatchWriteItem",
            body: json!({"RequestItems": {CEILINGS: [
                {"PutRequest": {"Item": sized_item("batched", payload_for("batched", over))}}
            ]}}),
            expected: ITEM_TOO_BIG.to_string(),
            probe: Some("batched"),
        },
        Ceiling {
            case: "the same item inside a transaction",
            operation: "TransactWriteItems",
            body: json!({"TransactItems": [
                {"Put": {"TableName": CEILINGS,
                         "Item": sized_item("transacted", payload_for("transacted", over))}}
            ]}),
            expected: ITEM_TOO_BIG.to_string(),
            probe: Some("transacted"),
        },
        Ceiling {
            case: "a value nesting one level past the nesting ceiling",
            operation: "PutItem",
            body: json!({"TableName": CEILINGS, "Item": {"pk": {"S": "deep"}, "tree": deep}}),
            expected: format!("{ENVELOPE}{NESTING}"),
            probe: Some("deep"),
        },
        Ceiling {
            case: "a value nesting far past it states the same sentence",
            operation: "PutItem",
            body: json!({"TableName": CEILINGS, "Item": {"pk": {"S": "deeper"}, "tree": deeper}}),
            expected: format!("{ENVELOPE}{NESTING}"),
            probe: Some("deeper"),
        },
        Ceiling {
            case: "the same value inside a batched write",
            operation: "BatchWriteItem",
            body: json!({"RequestItems": {CEILINGS: [
                {"PutRequest": {"Item": {"pk": {"S": "batch-deep"}, "tree": nested(MAX_DEPTH + 1)}}}
            ]}}),
            expected: NESTING.to_string(),
            probe: Some("batch-deep"),
        },
        Ceiling {
            case: "the same value inside a transaction",
            operation: "TransactWriteItems",
            body: json!({"TransactItems": [
                {"Put": {"TableName": CEILINGS,
                         "Item": {"pk": {"S": "tx-deep"}, "tree": nested(MAX_DEPTH + 1)}}}
            ]}),
            expected: NESTING.to_string(),
            probe: Some("tx-deep"),
        },
        Ceiling {
            case: "a partition key one byte past its documented size limit",
            operation: "PutItem",
            body: json!({"TableName": CEILINGS, "Item": {"pk": {"S": long_key.clone()}}}),
            expected: format!(
                "{INVALID_PARAMETER}: Size of hashkey has exceeded the maximum size limit \
                 of{MAX_PARTITION_KEY_BYTES} bytes"
            ),
            probe: None,
        },
        Ceiling {
            case: "the same key read back through a keyed read",
            operation: "GetItem",
            body: json!({"TableName": CEILINGS, "Key": {"pk": {"S": long_key}}}),
            expected: format!(
                "{INVALID_PARAMETER}: Size of hashkey has exceeded the maximum size limit \
                 of{MAX_PARTITION_KEY_BYTES} bytes"
            ),
            probe: None,
        },
    ];

    for row in rows {
        let stated = refused(&router, row.case, row.operation, row.body).await;
        assert_eq!(
            stated, row.expected,
            "case {:?} ({}): expected the message {:?}, got {stated:?}",
            row.case, row.operation, row.expected
        );
        if let Some(pk) = row.probe {
            assert!(
                absent(&router, CEILINGS, pk).await,
                "case {:?} ({}): a refused write stores nothing, but {pk:?} is now readable",
                row.case,
                row.operation
            );
        }
    }
}

#[tokio::test]
async fn a_write_on_each_published_ceiling_is_served() {
    let router = emulator();
    partitioned(&router, "DomainCeilingsBounds").await;
    let table = "DomainCeilingsBounds";

    // Exactly AT each documented ceiling: the item that stores 409,600 bytes, the value
    // nesting 32 levels, and the 2048-byte partition key — so every refusal above is the
    // bound and not an off-by-one.
    for (case, item, pk) in [
        (
            "an item storing exactly the documented ceiling",
            sized_item("edge", payload_for("edge", MAX_ITEM_BYTES)),
            "edge",
        ),
        (
            "a value nesting exactly the documented levels",
            json!({"pk": {"S": "edge-deep"}, "tree": nested(MAX_DEPTH - 1)}),
            "edge-deep",
        ),
        (
            "a partition key of exactly the documented size",
            json!({"pk": {"S": "e".repeat(MAX_PARTITION_KEY_BYTES)}}),
            "",
        ),
    ] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": item.clone()}),
        )
        .await;
        if !pk.is_empty() {
            assert!(
                !absent(&router, table, pk).await,
                "case {case:?}: a write on the published bound is stored, but {pk:?} reads back \
                 as absent"
            );
        }
    }
}

#[tokio::test]
async fn a_transaction_past_the_aggregate_payload_ceiling_is_refused_whole() {
    let router = emulator();
    partitioned(&router, "DomainCeilingsTransaction").await;
    let table = "DomainCeilingsTransaction";

    // "The aggregate size of the items in the transaction cannot exceed 4 MB"
    // ("Service, Account, and Table Quotas"): the ceiling is on the whole CALL, so every
    // action here is well inside the 400 KB item ceiling and only their sum crosses it.
    let per_action = 380 * 1024;
    let actions = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|at| {
                    let pk = format!("agg-{at}");
                    let payload = payload_for(&pk, per_action);
                    json!({"Put": {"TableName": table, "Item": sized_item(&pk, payload)}})
                })
                .collect(),
        )
    };
    let aggregate = |count: usize| count * per_action;
    let over = 4 * 1024 * 1024 / per_action + 1;
    assert!(
        aggregate(over) > 4 * 1024 * 1024 && aggregate(over - 1) <= 4 * 1024 * 1024,
        "the row must straddle the 4 MB ceiling: {over} actions of {per_action} bytes"
    );

    let stated = refused(
        &router,
        "a transaction past the 4 MB aggregate ceiling",
        "TransactWriteItems",
        json!({"TransactItems": actions(over)}),
    )
    .await;
    // The sentence states the ceiling as the quota publishes it and then the payload this
    // call came to, which is the one figure a caller sizing or splitting a transaction has
    // to read; the figure is the aggregate, so it is read back off the message rather than
    // transcribed.
    let prefix = "Transaction payload size cannot exceed 4MB. Payload Size: ";
    let reported: Option<i64> = stated.strip_prefix(prefix).and_then(|tail| tail.parse().ok());
    assert!(
        reported.is_some_and(|bytes| bytes > 4 * 1024 * 1024),
        "case a transaction past the 4 MB aggregate ceiling: expected {prefix:?} followed by a \
         payload past 4 MB, got {stated:?}"
    );
    for at in 0..over {
        let pk = format!("agg-{at}");
        assert!(
            absent(&router, table, &pk).await,
            "case a transaction past the 4 MB aggregate ceiling: nothing of a refused \
             transaction is stored, but {pk:?} is now readable"
        );
    }

    // The CONTROL: the same actions one short of the ceiling are applied, so the refusal
    // above is the aggregate and not the shape of the transaction.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": actions(over - 1)}),
    )
    .await;
    assert!(
        !absent(&router, table, "agg-0").await,
        "case a transaction inside the aggregate ceiling: every action is applied, but \
         \"agg-0\" reads back as absent"
    );
}

// =============================================================================
// THE NUMERIC DOMAIN
// =============================================================================

/// One number row: the text an `N` slot carries and the sentence it earns.
struct Number {
    case: &'static str,
    text: String,
    clause: String,
}

const NUMBERS: &str = "DomainNumbers";

/// The sentences the numeric domain states, each AWS's fixed text for that class.
fn precision_clause() -> String {
    format!("Attempting to store more than {MAX_SIGNIFICANT_DIGITS} significant digits in a Number")
}
const OVERFLOW_CLAUSE: &str =
    "Number overflow. Attempting to store a number with magnitude larger than supported range";
const UNDERFLOW_CLAUSE: &str =
    "Number underflow. Attempting to store a number with magnitude smaller than supported range";
fn conversion_clause(text: &str) -> String {
    format!("The parameter cannot be converted to a numeric value: {text}")
}

#[tokio::test]
async fn the_numeric_domain_is_enforced_wherever_a_number_arrives() {
    let router = emulator();
    partitioned(&router, NUMBERS).await;

    // The rows differ along which half of the domain the text leaves — the precision
    // ceiling, each end of the exponent range, and a text that denotes no decimal number
    // at all — and along how it is written: as digits, with an exponent, with a sign, and
    // as a zero whose exponent alone is out of range.
    let rows: Vec<Number> = vec![
        Number {
            case: "one significant digit past the precision ceiling",
            text: "1".repeat(MAX_SIGNIFICANT_DIGITS + 1),
            clause: precision_clause(),
        },
        Number {
            case: "far past the precision ceiling",
            text: "9".repeat(MAX_SIGNIFICANT_DIGITS + 12),
            clause: precision_clause(),
        },
        Number {
            case: "a fraction past the precision ceiling",
            text: format!("0.{}", "7".repeat(MAX_SIGNIFICANT_DIGITS + 1)),
            clause: precision_clause(),
        },
        Number {
            case: "a negative number past the precision ceiling",
            text: format!("-{}", "3".repeat(MAX_SIGNIFICANT_DIGITS + 1)),
            clause: precision_clause(),
        },
        Number {
            case: "one exponent past the documented maximum",
            text: format!("1e{}", MAX_EXPONENT + 1),
            clause: OVERFLOW_CLAUSE.to_string(),
        },
        Number {
            case: "one exponent past the documented minimum",
            text: format!("1e{}", MIN_EXPONENT - 1),
            clause: UNDERFLOW_CLAUSE.to_string(),
        },
        Number {
            case: "zero written with an exponent past the documented maximum",
            text: format!("0e{}", MAX_EXPONENT + 1),
            clause: OVERFLOW_CLAUSE.to_string(),
        },
        Number {
            case: "zero written with an exponent past the documented minimum",
            text: format!("0e{}", MIN_EXPONENT - 1),
            clause: UNDERFLOW_CLAUSE.to_string(),
        },
        Number {
            case: "a text that is no number at all",
            text: "abc".to_string(),
            clause: conversion_clause("abc"),
        },
        Number {
            case: "a text whose exponent is no number",
            text: "1e".to_string(),
            clause: conversion_clause("1e"),
        },
    ];

    for row in &rows {
        // An item attribute of a single-item write states the clause inside the counting
        // envelope, which is the form a clause about a value an item-API member carried
        // takes.
        let stated = refused(
            &router,
            row.case,
            "PutItem",
            json!({
                "TableName": NUMBERS,
                "Item": {"pk": {"S": "n"}, "v": {"N": row.text}},
            }),
        )
        .await;
        assert_eq!(
            stated,
            format!("{ENVELOPE}{}", row.clause),
            "case {:?} as an item attribute: expected the clause {:?} in the validation \
             envelope, got {stated:?}",
            row.case,
            row.clause
        );
        assert!(
            absent(&router, NUMBERS, "n").await,
            "case {:?}: a refused write stores nothing, but the item is now readable",
            row.case
        );

        // A PartiQL PARAMETER binds the same domain, and a statement states its clause on
        // its own: the rule is the numeric domain's, not one surface's.
        let stated = refused(
            &router,
            row.case,
            "ExecuteStatement",
            json!({
                "Statement": format!("SELECT * FROM \"{NUMBERS}\" WHERE pk = ?"),
                "Parameters": [{"N": row.text}],
            }),
        )
        .await;
        assert_eq!(
            stated, row.clause,
            "case {:?} as a statement parameter: expected the clause alone, got {stated:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_number_inside_the_documented_domain_is_served() {
    let router = emulator();
    partitioned(&router, "DomainNumbersBounds").await;
    let table = "DomainNumbersBounds";

    // Each published bound, and the one case the rule generalises over that a width-based
    // reading would get wrong: a whole number written with 41 digits of TEXT whose
    // significant digits are one, which the domain counts rather than the text.
    for (case, text) in [
        ("the precision ceiling itself", "1".repeat(MAX_SIGNIFICANT_DIGITS)),
        ("the documented maximum exponent", format!("1e{MAX_EXPONENT}")),
        ("the documented minimum exponent", format!("1e{MIN_EXPONENT}")),
        ("one significant digit behind a long run of zeros", format!("1{}", "0".repeat(40))),
        ("zero", "0".to_string()),
    ] {
        let written = ok(
            &router,
            "PutItem",
            json!({
                "TableName": table,
                "Item": {"pk": {"S": case}, "v": {"N": text}},
                "ReturnConsumedCapacity": "TOTAL",
            }),
        )
        .await;
        assert_eq!(
            written["ConsumedCapacity"]["TableName"],
            json!(table),
            "case {case:?}: a number inside the domain is written, got {written}"
        );
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": table, "Key": {"pk": {"S": case}}}),
        )
        .await;
        assert!(
            read["Item"]["v"].get("N").is_some(),
            "case {case:?}: the stored number reads back as a number, got {read}"
        );
    }
}

// =============================================================================
// THE SET-MEMBER DOMAIN
// =============================================================================

const SETS: &str = "DomainSets";

#[tokio::test]
async fn a_set_may_not_repeat_a_member_wherever_a_request_carries_one() {
    let router = emulator();
    partitioned(&router, SETS).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": SETS, "Item": {"pk": {"S": "seed"}, "tags": {"SS": ["a", "b"]}}}),
    )
    .await;

    // One row per set type, each with the wording DynamoDB states for it — down to the
    // comma spacing of the string form, the number form that names no member at all, and
    // the binary form's missing space after the bracket. The number row repeats a member
    // SPELLED two ways, because distinctness is decided on the numbers and not on their
    // texts.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a string set repeating a member",
            json!({"SS": ["a", "a"]}),
            format!("{INVALID_PARAMETER}: Input collection [\"a\", \"a\"] contains duplicates."),
        ),
        (
            "a number set repeating a member spelled two ways",
            json!({"NS": ["1", "1.0"]}),
            format!("{INVALID_PARAMETER}: Input collection contains duplicates."),
        ),
        (
            "a binary set repeating a member",
            json!({"BS": ["YWJj", "YWJj"]}),
            format!(
                "{INVALID_PARAMETER}: Input collection [\"YWJj\",\"YWJj\"]of type BS contains \
                 duplicates."
            ),
        ),
        (
            "an empty string set",
            json!({"SS": []}),
            format!("{INVALID_PARAMETER}: An string set  may not be empty"),
        ),
    ];

    for (case, value, clause) in &rows {
        // As an ITEM attribute of a write, at the top level and nested inside a list, and
        // as an EXPRESSION value — the write states its clause inside the counting
        // envelope, and the read that narrows its result with one states it alone.
        for (surface, operation, body, enveloped) in [
            (
                "an item attribute",
                "PutItem",
                json!({"TableName": SETS, "Item": {"pk": {"S": "set"}, "v": value}}),
                true,
            ),
            (
                "a set nested inside a list",
                "PutItem",
                json!({"TableName": SETS,
                       "Item": {"pk": {"S": "set"}, "v": {"L": [{"S": "ok"}, value]}}}),
                true,
            ),
            (
                "a condition value of a write",
                "PutItem",
                json!({
                    "TableName": SETS,
                    "Item": {"pk": {"S": "set"}},
                    "ConditionExpression": "tags = :v",
                    "ExpressionAttributeValues": {":v": value},
                }),
                true,
            ),
            (
                "a filter value of a read",
                "Scan",
                json!({
                    "TableName": SETS,
                    "FilterExpression": "tags = :v",
                    "ExpressionAttributeValues": {":v": value},
                }),
                false,
            ),
        ] {
            let full = format!("{case} as {surface}");
            let stated = refused(&router, &full, operation, body).await;
            let expected = if enveloped {
                format!("{ENVELOPE}{clause}")
            } else {
                clause.clone()
            };
            assert_eq!(
                stated, expected,
                "case {full:?}: expected the message {expected:?}, got {stated:?}"
            );
            assert!(
                absent(&router, SETS, "set").await,
                "case {full:?}: a refused write stores nothing, but the item is now readable"
            );
        }
    }

    // The CONTROL: the same three set types with DISTINCT, non-empty members are written
    // and read back, so each refusal above is the repeated member and not the set.
    for (case, value) in [
        ("a string set of distinct members", json!({"SS": ["a", "b"]})),
        ("a number set of distinct members", json!({"NS": ["1", "2"]})),
        ("a binary set of distinct members", json!({"BS": ["YWJj", "YWJk"]})),
    ] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": SETS, "Item": {"pk": {"S": case}, "v": value}}),
        )
        .await;
        assert!(
            !absent(&router, SETS, case).await,
            "case {case:?}: a set whose members are distinct is stored, but the item reads back \
             as absent"
        );
    }
}

// =============================================================================
// THE READ-REQUEST DEFECTS
// =============================================================================

const READS: &str = "DomainReads";

/// The table the read rows address: a partition and a sort key, so a key condition can
/// bound a range and a resume token can fall outside it.
async fn read_table(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": READS,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
            ],
        }),
    )
    .await;
    describe_when_active(router, READS, READS).await;
    for sk in ["s1", "s2", "s3"] {
        ok(
            router,
            "PutItem",
            json!({"TableName": READS,
                   "Item": {"pk": {"S": "p"}, "sk": {"S": sk}, "note": {"S": sk}}}),
        )
        .await;
    }
}

#[tokio::test]
async fn a_read_request_outside_its_documented_bounds_is_refused() {
    let router = emulator();
    read_table(&router).await;

    let operands = |count: usize| -> (String, Value) {
        let placeholders: Vec<String> = (0..count).map(|at| format!(":v{at}")).collect();
        let values: serde_json::Map<String, Value> = placeholders
            .iter()
            .enumerate()
            .map(|(at, name)| (name.clone(), json!({"S": format!("v{at}")})))
            .collect();
        (
            format!("note IN ({})", placeholders.join(", ")),
            Value::Object(values),
        )
    };

    // Each row names the documented bound it crosses and the sentence that bound is
    // stated with. The bound itself is driven below, so none of these is an off-by-one.
    let (over_operands, over_values) = operands(101);
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            // "The IN operator can have up to 100 comparison values" (the
            // `ConditionExpression` comparator reference).
            "an IN list one operand past the 100-value bound",
            "Scan",
            json!({"TableName": READS, "FilterExpression": over_operands,
                   "ExpressionAttributeValues": over_values}),
            "Invalid FilterExpression: The IN operator is provided with too many operands; \
             number of operands: 101"
                .to_string(),
        ),
        (
            "a resume token that gives no value for a key attribute",
            "Query",
            json!({
                "TableName": READS,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "p"}},
                "ExclusiveStartKey": {"pk": {"S": "p"}},
            }),
            "The provided starting key is invalid".to_string(),
        ),
        (
            "a resume token carrying an attribute the schema declares no key for",
            "Query",
            json!({
                "TableName": READS,
                "KeyConditionExpression": "pk = :p",
                "ExpressionAttributeValues": {":p": {"S": "p"}},
                "ExclusiveStartKey": {"pk": {"S": "p"}, "sk": {"S": "s1"}, "note": {"S": "s1"}},
            }),
            "The provided starting key is invalid".to_string(),
        ),
        (
            "a resume token outside the range the key condition selects",
            "Query",
            json!({
                "TableName": READS,
                "KeyConditionExpression": "pk = :p AND sk < :s",
                "ExpressionAttributeValues": {":p": {"S": "p"}, ":s": {"S": "s2"}},
                "ExclusiveStartKey": {"pk": {"S": "p"}, "sk": {"S": "s3"}},
            }),
            "The provided starting key does not match the range key predicate".to_string(),
        ),
        (
            // Smithy models `Scan$TotalSegments` with `@range{min:1,max:1000000}`
            // (`smithy_query(query="operation", operation="Scan")`), and a table walk
            // names the member by its WIRE spelling with no value echoed.
            "a parallel scan one segment past the modelled maximum",
            "Scan",
            json!({"TableName": READS, "Segment": 0, "TotalSegments": 1_000_001}),
            format!(
                "{ENVELOPE}Value at 'TotalSegments' failed to satisfy constraint: Member must \
                 have value less than or equal to 1000000"
            ),
        ),
        (
            "a parallel scan one segment below the modelled minimum",
            "Scan",
            json!({"TableName": READS, "Segment": 0, "TotalSegments": 0}),
            format!(
                "{ENVELOPE}Value at 'TotalSegments' failed to satisfy constraint: Member must \
                 have value greater than or equal to 1"
            ),
        ),
        (
            // An endpoint serves ONE region, so an ARN naming another names nothing this
            // endpoint can reach — an input failure and not a missing table.
            "a table addressed by another region's ARN",
            "DescribeTable",
            json!({"TableName": state::table_arn("us-west-2", READS)}),
            format!(
                "{ENVELOPE}Invalid AWS region in '{}'",
                state::table_arn("us-west-2", READS)
            ),
        ),
    ];

    for (case, operation, body, expected) in rows {
        let stated = refused(&router, case, operation, body).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // The projection rules `Select` states, whose sentences name the MEMBERS a projection
    // can be listed in and the one place an index projection can be reported from.
    for (case, body, core) in [
        (
            "SPECIFIC_ATTRIBUTES with no projection listed",
            json!({"TableName": READS, "Select": "SPECIFIC_ATTRIBUTES"}),
            "when choosing to get SPECIFIC_ATTRIBUTES",
        ),
        (
            "ALL_PROJECTED_ATTRIBUTES on a base-table walk",
            json!({"TableName": READS, "Select": "ALL_PROJECTED_ATTRIBUTES"}),
            "ALL_PROJECTED_ATTRIBUTES can be used only when Querying using an IndexName",
        ),
    ] {
        let stated = refused(&router, case, "Scan", body).await;
        assert!(
            stated.contains(core),
            "case {case:?}: expected a refusal stating {core:?}, got {stated:?}"
        );
    }

    // Every bound itself, and the ARN of THIS region: served.
    let (at_bound, at_values) = operands(100);
    ok(
        &router,
        "Scan",
        json!({"TableName": READS, "FilterExpression": at_bound,
               "ExpressionAttributeValues": at_values}),
    )
    .await;
    for (case, body) in [
        (
            "a parallel scan at the modelled maximum",
            json!({"TableName": READS, "Segment": 0, "TotalSegments": 1_000_000}),
        ),
        (
            "a parallel scan at the modelled minimum",
            json!({"TableName": READS, "Segment": 0, "TotalSegments": 1}),
        ),
    ] {
        let answered = ok(&router, "Scan", body).await;
        assert!(
            answered.get("Count").is_some(),
            "case {case:?}: a scan at the published bound is served, got {answered}"
        );
    }
    let described = ok(
        &router,
        "DescribeTable",
        json!({"TableName": state::table_arn(REGION, READS)}),
    )
    .await;
    assert_eq!(
        described["Table"]["TableName"],
        json!(READS),
        "case this region's own ARN: expected the table, got {described}"
    );
    // And a resume token the condition DOES select pages from, so each token refusal above
    // is where the key fell and not the token's presence.
    let paged = ok(
        &router,
        "Query",
        json!({
            "TableName": READS,
            "KeyConditionExpression": "pk = :p",
            "ExpressionAttributeValues": {":p": {"S": "p"}},
            "ExclusiveStartKey": {"pk": {"S": "p"}, "sk": {"S": "s1"}},
        }),
    )
    .await;
    assert_eq!(
        paged["Count"],
        json!(2),
        "case a resume token inside the range: expected the two items past it, got {paged}"
    );
}

// =============================================================================
// THE STATEMENT SEMANTICS
// =============================================================================

const STATEMENTS: &str = "DomainStatements";

#[tokio::test]
async fn a_statement_reads_a_missing_attribute_as_missing_and_removes_nothing_from_nothing() {
    let router = emulator();
    partitioned(&router, STATEMENTS).await;
    // Three items: one carrying the NULL-typed value, one carrying another value, one
    // carrying no such attribute at all — the three answers `IS NULL` has to tell apart.
    for (pk, attribute) in [
        ("typed-null", Some(json!({"NULL": true}))),
        ("valued", Some(json!({"S": "x"}))),
        ("absent", None),
    ] {
        let mut item = json!({"pk": {"S": pk}});
        if let Some(value) = attribute {
            item["a"] = value;
        }
        ok(
            &router,
            "PutItem",
            json!({"TableName": STATEMENTS, "Item": item}),
        )
        .await;
    }
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": STATEMENTS,
            "Item": {
                "pk": {"S": "nested"},
                "m": {"M": {"leaf": {"S": "deep"}}},
                "l": {"L": [{"N": "100"}, {"N": "200"}]},
            },
        }),
    )
    .await;

    // "a IS NULL" asks about the TYPE of a stored attribute, so an attribute the item does
    // not carry is MISSING rather than null: it satisfies neither `IS NULL` nor the
    // positive half, and satisfies the NEGATION of `IS NULL` because it is not a stored
    // null. `IS MISSING` is the clause that asks about absence.
    for (case, clause, expected) in [
        ("IS NULL names only the stored null", "a IS NULL", vec!["typed-null"]),
        (
            "IS NOT NULL names everything that is not a stored null",
            "a IS NOT NULL",
            vec!["absent", "nested", "valued"],
        ),
        ("IS MISSING names the absence", "a IS MISSING", vec!["absent", "nested"]),
        (
            "IS NOT MISSING names what carries the attribute",
            "a IS NOT MISSING",
            vec!["typed-null", "valued"],
        ),
    ] {
        let answered = ok(
            &router,
            "ExecuteStatement",
            json!({"Statement": format!("SELECT pk FROM \"{STATEMENTS}\" WHERE {clause}")}),
        )
        .await;
        let mut keys: Vec<String> = answered["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|item| item["pk"]["S"].as_str().map(ToString::to_string))
            .collect();
        keys.sort();
        assert_eq!(
            keys, expected,
            "case {case:?}: expected {expected:?}, got {keys:?} from {answered}"
        );
    }

    // A SELECT that projects a document path reports the value under the path from its
    // LAST NAMED step on — the name, and the `[index]` steps into it — so two elements of
    // one list are two entries of the answer rather than one shared key, and a caller can
    // correlate each value back to the element it asked for.
    let projected = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": format!(
            "SELECT m.leaf, l[0], l[1] FROM \"{STATEMENTS}\" WHERE pk = 'nested'"
        )}),
    )
    .await;
    assert_eq!(
        projected["Items"],
        json!([{"leaf": {"S": "deep"}, "l[0]": {"N": "100"}, "l[1]": {"N": "200"}}]),
        "case a projected document path: expected each value under the path from its last \
         named step on, got {projected}"
    );

    // A removal that addresses a key no item is stored under changes nothing and CREATES
    // nothing — through the statement surface and through the item API alike — while an
    // assignment to the same absent key DOES add the item it assigns to: what decides is
    // the clause, not the absence.
    for (case, pk, operation, body) in [
        (
            "a statement removing an attribute of an absent key",
            "nowhere",
            "ExecuteStatement",
            json!({"Statement": format!(
                "UPDATE \"{STATEMENTS}\" REMOVE a WHERE pk = 'nowhere' RETURNING ALL NEW *"
            )}),
        ),
        (
            "an update whose only clause removes an attribute of an absent key",
            "nowhere-item",
            "UpdateItem",
            json!({
                "TableName": STATEMENTS,
                "Key": {"pk": {"S": "nowhere-item"}},
                "UpdateExpression": "REMOVE a",
                "ReturnValues": "ALL_NEW",
            }),
        ),
        (
            "the same removal stated through the legacy update map",
            "nowhere-legacy",
            "UpdateItem",
            json!({
                "TableName": STATEMENTS,
                "Key": {"pk": {"S": "nowhere-legacy"}},
                "AttributeUpdates": {"a": {"Action": "DELETE"}},
                "ReturnValues": "ALL_NEW",
            }),
        ),
    ] {
        let (status, answered) = call(&router, operation, body).await;
        assert!(
            absent(&router, STATEMENTS, pk).await,
            "case {case:?} ({operation}): a removal creates nothing, but {pk:?} is now \
             readable (the call answered {status} with {answered})"
        );
        assert!(
            answered.get("Attributes").is_none() && answered["Items"].as_array().is_none(),
            "case {case:?} ({operation}): there is no item to report, got {answered}"
        );
    }

    let created = ok(
        &router,
        "UpdateItem",
        json!({
            "TableName": STATEMENTS,
            "Key": {"pk": {"S": "created"}},
            "UpdateExpression": "SET a = :v",
            "ExpressionAttributeValues": {":v": {"S": "set"}},
            "ReturnValues": "ALL_NEW",
        }),
    )
    .await;
    assert_eq!(
        created["Attributes"]["a"],
        json!({"S": "set"}),
        "case an assignment to an absent key: the item is created, got {created}"
    );
    assert!(
        !absent(&router, STATEMENTS, "created").await,
        "case an assignment to an absent key: the created item reads back"
    );
}

// =============================================================================
// WHAT AN INDEX-QUALIFIED STATEMENT MAY FILTER ON
// =============================================================================

const INDEXED: &str = "DomainIndexed";

/// The global secondary index that projects ONE non-key attribute and not the other.
const INCLUDE_GSI: &str = "by-gpk";

/// The local secondary index that projects the keys alone.
const KEYS_ONLY_LSI: &str = "by-lsk";

#[tokio::test]
async fn a_statement_may_not_filter_on_what_its_index_does_not_project() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": INDEXED,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gpk", "AttributeType": "S"},
                {"AttributeName": "lsk", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": INCLUDE_GSI,
                "KeySchema": [{"AttributeName": "gpk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "INCLUDE", "NonKeyAttributes": ["shown"]},
            }],
            "LocalSecondaryIndexes": [{
                "IndexName": KEYS_ONLY_LSI,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "lsk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
            }],
        }),
    )
    .await;
    describe_when_active(&router, INDEXED, INDEXED).await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": INDEXED,
            "Item": {
                "pk": {"S": "p"},
                "sk": {"S": "s"},
                "gpk": {"S": "g"},
                "lsk": {"S": "l"},
                "shown": {"S": "projected"},
                "hidden": {"S": "unprojected"},
            },
        }),
    )
    .await;

    // The rule is the index's PROJECTION: a statement served as a query of an index — one
    // that states a condition on that index's partition key — may only filter on what the
    // index holds for the item. The rows span both index kinds and both projection types
    // that hold back an attribute.
    for (case, index, filter, unprojected) in [
        (
            "a non-projected attribute of an INCLUDE global index",
            INCLUDE_GSI,
            "gpk = 'g' AND hidden = 'unprojected'",
            "hidden",
        ),
        (
            "another non-projected attribute of the same index",
            INCLUDE_GSI,
            "gpk = 'g' AND sk = 's' AND hidden = 'unprojected'",
            "hidden",
        ),
        (
            "a non-projected attribute of a KEYS_ONLY local index",
            KEYS_ONLY_LSI,
            "pk = 'p' AND hidden = 'unprojected'",
            "hidden",
        ),
        (
            "a non-key attribute the KEYS_ONLY index does not hold either",
            KEYS_ONLY_LSI,
            "pk = 'p' AND shown = 'projected'",
            "shown",
        ),
    ] {
        let stated = refused(
            &router,
            case,
            "ExecuteStatement",
            json!({"Statement": format!(
                "SELECT * FROM \"{INDEXED}\".\"{index}\" WHERE {filter}"
            )}),
        )
        .await;
        assert_eq!(
            stated,
            format!(
                "{INVALID_PARAMETER}: Secondary index {index} does not project one or more \
                 filter attributes: [{unprojected}]"
            ),
            "case {case:?}: expected the index's own projection refusal, got {stated:?}"
        );
    }

    // The CONTROL: through the same qualifiers, a filter on what each index DOES project
    // is served — so the refusals above are the projection and not the qualifier.
    for (case, index, filter) in [
        (
            "a projected attribute of the INCLUDE global index",
            INCLUDE_GSI,
            "gpk = 'g' AND shown = 'projected'",
        ),
        (
            "the key attributes of the KEYS_ONLY local index",
            KEYS_ONLY_LSI,
            "pk = 'p' AND lsk = 'l'",
        ),
    ] {
        let answered = ok(
            &router,
            "ExecuteStatement",
            json!({"Statement": format!(
                "SELECT * FROM \"{INDEXED}\".\"{index}\" WHERE {filter}"
            )}),
        )
        .await;
        assert_eq!(
            answered["Items"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: a filter on what the index projects is served, got {answered}"
        );
    }
}

// =============================================================================
// THE VECTOR SCHEMA AND ITS METER
// =============================================================================

const VECTORS: &str = "DomainVectors";

/// The dimensions the table's vector index is created with.
const DIMENSIONS: usize = 4;

/// The per-write byte floor a vector write is metered at
/// (`state::MIN_VECTOR_REQUEST_BYTES`).
const VECTOR_FLOOR: f64 = 1024.0;

/// A vector attribute of `count` components, in the list form a vector is written in.
fn vector_of(count: usize) -> Value {
    Value::Array((0..count).map(|at| json!({"N": at.to_string()})).collect())
}

#[tokio::test]
async fn a_vector_write_is_held_to_its_index_and_metered_with_the_published_floor() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": VECTORS,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "VectorIndexes": [{
                "IndexName": "emb-idx",
                "VectorAttribute": {"AttributeName": "emb"},
                "Dimensions": DIMENSIONS,
                "DistanceFunction": "COSINE",
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    describe_when_active(&router, VECTORS, VECTORS).await;

    // "The number of dimensions must match the number of dimensions configured for the
    // vector index", and each component must be a value a 32-bit float holds: the rows
    // span one component short, one long, none at all, a component that is no number, and
    // an attribute that is no list.
    for (case, value) in [
        ("one component short", json!({"L": vector_of(DIMENSIONS - 1)})),
        ("one component long", json!({"L": vector_of(DIMENSIONS + 1)})),
        ("no component at all", json!({"L": []})),
        (
            "a component that is no number",
            json!({"L": [{"N": "0"}, {"S": "x"}, {"N": "2"}, {"N": "3"}]}),
        ),
        ("an attribute that is no list", json!({"S": "not-a-vector"})),
    ] {
        let stated = refused(
            &router,
            case,
            "PutItem",
            json!({"TableName": VECTORS, "Item": {"pk": {"S": "v"}, "emb": value}}),
        )
        .await;
        assert!(
            stated.contains("emb") && stated.contains("emb-idx"),
            "case {case:?}: the refusal names the vector attribute and the index that requires \
             it, got {stated:?}"
        );
        assert!(
            absent(&router, VECTORS, "v").await,
            "case {case:?}: a refused vector write stores nothing, but the item is now readable"
        );
    }

    // A vector write is metered per index over that attribute, with the published
    // per-write byte floor: a small entry meters the floor, and an entry whose own bytes
    // exceed it meters those bytes instead — which is what makes the figure a function of
    // the index rather than a constant.
    let written = ok(
        &router,
        "PutItem",
        json!({
            "TableName": VECTORS,
            "Item": {"pk": {"S": "small"}, "emb": {"L": vector_of(DIMENSIONS)}},
            "ReturnConsumedCapacity": "INDEXES",
        }),
    )
    .await;
    let metered = written["ConsumedCapacity"]["VectorIndexes"]["emb-idx"]
        ["VectorWriteRequestBytes"]
        .as_f64();
    assert_eq!(
        metered,
        Some(VECTOR_FLOOR),
        "case a small vector entry: expected the published {VECTOR_FLOOR}-byte floor, got \
         {written}"
    );

    let padded = "p".repeat(4 * 1024);
    let bigger = ok(
        &router,
        "PutItem",
        json!({
            "TableName": VECTORS,
            "Item": {
                "pk": {"S": "large"},
                "emb": {"L": vector_of(DIMENSIONS)},
                "pad": {"S": padded},
            },
            "ReturnConsumedCapacity": "INDEXES",
        }),
    )
    .await;
    let metered = bigger["ConsumedCapacity"]["VectorIndexes"]["emb-idx"]["VectorWriteRequestBytes"]
        .as_f64();
    assert!(
        metered.is_some_and(|bytes| bytes > VECTOR_FLOOR),
        "case an entry past the floor: expected more than {VECTOR_FLOOR} bytes, got {bigger}"
    );
}

// =============================================================================
// THE BATCHED WRITE'S REQUEST SHAPE
// =============================================================================

const BATCH: &str = "DomainBatch";

/// `count` write requests for that table, each a put of a distinct item.
fn write_requests(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|at| json!({"PutRequest": {"Item": {"pk": {"S": format!("b{at}")}}}}))
            .collect(),
    )
}

#[tokio::test]
async fn a_batched_write_states_its_modelled_bounds_and_absorbs_a_full_batch() {
    let router = emulator();
    partitioned(&router, BATCH).await;

    // Smithy models `BatchWriteItemInput$RequestItems` with `@length{min:1,max:25}` over
    // the write requests it carries (`smithy_query(query="operation",
    // operation="BatchWriteItem")`), and this operation names the member by the path the
    // request validator builds. The rows cross each end of that bound, through a map with
    // no entry at all — the MAP's own bound, whose clause states the one end that was
    // broken — and through a map whose entry is outside the bound declared for its VALUES,
    // whose clause states that value shape's whole set.
    let value_set = "Map value must satisfy constraint: [Member must have length less than or \
                     equal to 25, Member must have length greater than or equal to 1]";
    for (case, body, requirement) in [
        (
            "a batched write naming no table at all",
            json!({"RequestItems": {}}),
            "Member must have length greater than or equal to 1",
        ),
        (
            "a batched write whose only table was given no request",
            json!({"RequestItems": {BATCH: []}}),
            value_set,
        ),
        (
            "a batched write one request past the 25-request bound",
            json!({"RequestItems": {BATCH: write_requests(26)}}),
            value_set,
        ),
    ] {
        let stated = refused(&router, case, "BatchWriteItem", body).await;
        assert!(
            stated.starts_with(ENVELOPE)
                && stated.contains("at 'requestItems' failed to satisfy constraint: ")
                && stated.ends_with(requirement),
            "case {case:?}: expected the counted envelope over a clause naming 'requestItems' \
             and ending in {requirement:?}, got {stated:?}"
        );
    }

    // A batch filled to the published bound is ABSORBED WHOLE: every write lands and
    // nothing is handed back for the caller to retry, which is what a caller that trusts
    // an empty residue depends on.
    let answered = ok(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": {BATCH: write_requests(25)}}),
    )
    .await;
    assert_eq!(
        answered["UnprocessedItems"],
        json!({}),
        "case a batch at the published bound: expected an empty residue, got {answered}"
    );
    for at in 0..25 {
        let pk = format!("b{at}");
        assert!(
            !absent(&router, BATCH, &pk).await,
            "case a batch at the published bound: every write lands, but {pk:?} reads back as \
             absent (the batch answered {answered})"
        );
    }
}
