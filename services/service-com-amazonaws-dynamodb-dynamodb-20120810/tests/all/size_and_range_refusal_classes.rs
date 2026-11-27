// SPDX-License-Identifier: CC0-1.0

//! The CLASS a size or range refusal belongs to, driven end to end.
//!
//! Three rules under test, each stated over the SET of inputs it holds for rather than
//! over the one shape that showed it:
//!
//! 1. A `BETWEEN` draws ONE range, and DynamoDB orders values only within a type — so two
//!    bounds the request stated in DIFFERENT types draw no range at all and the call is a
//!    `ValidationException` naming that rule, with both bounds echoed as the operands they
//!    are. The rows span every expression member a `BETWEEN` can be written in and every
//!    ordered pair of the three types a key or a comparison orders, plus the BOUNDARY: two
//!    bounds of ONE type the wrong way round earn the other sentence, and two in the right
//!    order are evaluated.
//! 2. The documented 400 KB item ceiling is worded by the PATH that crossed it: a request
//!    that CARRIED an oversized item is told the item is too big, while a change that
//!    would GROW the stored item past it is told the item to update is. The rows span the
//!    surfaces that carry an item (`PutItem`, `BatchWriteItem`, a transaction's `Put`, a
//!    PartiQL `INSERT`) and the surfaces that state a CHANGE (`UpdateItem` through `SET`,
//!    `ADD` and `list_append`, a transaction's `Update`, a PartiQL `UPDATE`) — and WHERE
//!    the crossing is found decides whose failure it is: a VALUE the request carries that
//!    no item could hold is the request's own defect on every surface, while an item that
//!    crosses the ceiling only once the change is applied to what the table holds is, in a
//!    transaction, the crossing ACTION's own reason.
//! 3. A transaction is applied as one unit, so the documented 4 MB aggregate is a ceiling
//!    on the whole CALL: a transaction whose actions add up past it is refused as the
//!    malformed request it is, with nothing of it applied, while one whose actions add up
//!    under it is served however many actions it states.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: a string partition key and a string sort key, so a
/// `BETWEEN` row has a sort key to speak about.
const TABLE: &str = "SizeAndRangeClasses";

/// The envelope a refusal about the REQUEST's own write-intent members is stated inside —
/// this service's existing split, which each row below states the whole message through.
const ENVELOPE: &str = "1 validation error detected: ";

/// The documented item ceiling, 400 KB ("Service, Account, and Table Quotas": "The maximum
/// item size in DynamoDB is 400 KB").
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// AWS's fixed sentence for a request that carried an item past that ceiling.
const ITEM_TOO_BIG: &str = "Item size has exceeded the maximum allowed size";

/// AWS's fixed sentence for a change that would grow the stored item past it.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

fn key() -> Value {
    json!({"pk": {"S": "item-1"}, "sk": {"S": "a"}})
}

/// The emulator with the table and one item in place, so no row is refused for its table
/// or its key instead of for the rule under test.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S"), ("sk", "RANGE", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "item-1"}, "sk": {"S": "a"},
                        "n": {"N": "5"}, "s": {"S": "beta"}}}),
    )
    .await;
    router
}

/// The one refusal a request earns, asserted to be a `ValidationException` first so a row
/// that is answered with another class says so.
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

// ===========================================================================
// RULE 1. A BETWEEN's two bounds are of ONE type
// ===========================================================================

/// One attribute value as a refusal about an OPERAND echoes it: its type tag and the text
/// it carries, which is the form this family of messages states an operand in.
fn rendered(datatype: &str, text: &str) -> String {
    format!("AttributeValue: {{{datatype}:{text}}}")
}

/// AWS's sentence for a `BETWEEN` whose bounds are of two different types, behind the
/// member that carried the expression.
fn different_types(member: &str, low: (&str, &str), high: (&str, &str)) -> String {
    format!(
        "Invalid {member}: The BETWEEN operator requires same data type for lower and upper \
         bounds; lower bound operand: {}, upper bound operand: {}",
        rendered(low.0, low.1),
        rendered(high.0, high.1)
    )
}

/// The three types a comparison orders, as a well-formed value of each and as the text a
/// refusal echoes for it. `B` is echoed base64-encoded, which is how the wire carries it.
fn ordered_types() -> [(&'static str, Value, &'static str); 3] {
    [
        ("S", json!({"S": "m"}), "m"),
        ("N", json!({"N": "7"}), "7"),
        ("B", json!({"B": "YQ=="}), "YQ=="),
    ]
}

/// How one row below builds the call that writes its `BETWEEN`: the two bounds in, the
/// operation and its body out.
type Writes = fn(Value, Value) -> (&'static str, Value);

/// One row of the `BETWEEN` suite: the member the range is written in, the label the
/// assertions name it by, the call that writes it, and whether that member's refusals
/// carry the validation envelope.
struct Written {
    member: &'static str,
    label: &'static str,
    writes: Writes,
    enveloped: bool,
}

#[tokio::test]
async fn a_between_over_two_types_is_refused_naming_that_rule() {
    let router = seeded().await;

    let written: Vec<Written> = vec![
        Written {
            member: "ConditionExpression",
            label: "PutItem",
            writes: |low, high| {
                (
                    "PutItem",
                    json!({"TableName": TABLE,
                           "Item": {"pk": {"S": "item-1"}, "sk": {"S": "a"}},
                           "ConditionExpression": "n BETWEEN :lo AND :hi",
                           "ExpressionAttributeValues": {":lo": low, ":hi": high}}),
                )
            },
            enveloped: true,
        },
        Written {
            member: "ConditionExpression",
            label: "DeleteItem",
            writes: |low, high| {
                (
                    "DeleteItem",
                    json!({"TableName": TABLE, "Key": key(),
                           "ConditionExpression": "n BETWEEN :lo AND :hi",
                           "ExpressionAttributeValues": {":lo": low, ":hi": high}}),
                )
            },
            enveloped: true,
        },
        Written {
            member: "ConditionExpression",
            label: "UpdateItem",
            writes: |low, high| {
                (
                    "UpdateItem",
                    json!({"TableName": TABLE, "Key": key(),
                           "UpdateExpression": "SET s = :new",
                           "ConditionExpression": "n BETWEEN :lo AND :hi",
                           "ExpressionAttributeValues": {":lo": low, ":hi": high,
                                                         ":new": {"S": "x"}}}),
                )
            },
            enveloped: true,
        },
        Written {
            member: "FilterExpression",
            label: "Scan",
            writes: |low, high| {
                (
                    "Scan",
                    json!({"TableName": TABLE,
                           "FilterExpression": "n BETWEEN :lo AND :hi",
                           "ExpressionAttributeValues": {":lo": low, ":hi": high}}),
                )
            },
            enveloped: false,
        },
        Written {
            member: "KeyConditionExpression",
            label: "Query",
            writes: |low, high| {
                (
                    "Query",
                    json!({"TableName": TABLE,
                           "KeyConditionExpression": "pk = :p AND sk BETWEEN :lo AND :hi",
                           "ExpressionAttributeValues": {":p": {"S": "item-1"},
                                                         ":lo": low, ":hi": high}}),
                )
            },
            enveloped: false,
        },
    ];

    for row in &written {
        let (member, label) = (row.member, row.label);
        // Every ORDERED PAIR of the three types a comparison orders: the rule is about the
        // two bounds differing, whichever two types they are and whichever way round.
        for (low_type, low_value, low_text) in ordered_types() {
            for (high_type, high_value, high_text) in ordered_types() {
                if low_type == high_type {
                    continue;
                }
                let case = format!("{label}: {member} BETWEEN {low_type} and {high_type}");
                let (operation, body) = (row.writes)(low_value.clone(), high_value.clone());
                let stated = refusal(&router, &case, operation, body).await;
                let clause =
                    different_types(member, (low_type, low_text), (high_type, high_text));
                let expected = if row.enveloped {
                    format!("{ENVELOPE}{clause}")
                } else {
                    clause
                };
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected {expected:?}, got {stated:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_between_of_one_type_is_held_to_its_order_and_then_evaluated() {
    let router = seeded().await;

    // The BOUNDARY of rule 1: bounds of ONE type are in the comparator's own rule
    // instead — the lower bound must not sit above the upper one — and bounds in the
    // right order are evaluated against the stored item rather than refused.
    for (case, low, high) in [
        ("two numbers the wrong way round", json!({"N": "9"}), json!({"N": "1"})),
        ("two strings the wrong way round", json!({"S": "z"}), json!({"S": "a"})),
    ] {
        let stated = refusal(
            &router,
            case,
            "Scan",
            json!({"TableName": TABLE,
                   "FilterExpression": "n BETWEEN :lo AND :hi",
                   "ExpressionAttributeValues": {":lo": low.clone(), ":hi": high.clone()}}),
        )
        .await;
        assert!(
            stated.contains("requires upper bound to be greater than or equal to lower bound"),
            "case {case:?}: expected the bound-ORDER sentence, got {stated:?}"
        );
    }

    for (case, low, high, selects) in [
        ("a range the stored number falls inside", json!({"N": "1"}), json!({"N": "9"}), 1),
        ("a range the stored number falls outside", json!({"N": "6"}), json!({"N": "9"}), 0),
    ] {
        let answered = ok(
            &router,
            "Scan",
            json!({"TableName": TABLE,
                   "FilterExpression": "n BETWEEN :lo AND :hi",
                   "ExpressionAttributeValues": {":lo": low, ":hi": high}}),
        )
        .await;
        assert_eq!(
            answered["Count"],
            json!(selects),
            "case {case:?}: expected {selects} item(s), got {answered}"
        );
    }
}

// ===========================================================================
// RULE 2. The item ceiling is worded by the path that crossed it
// ===========================================================================

/// A string of `bytes` characters, for an attribute whose SIZE is the point of the row.
fn filler(bytes: usize) -> String {
    "f".repeat(bytes)
}

/// A value that, written into the seeded item, leaves it past the documented ceiling
/// whatever else the item already holds.
fn oversized() -> Value {
    json!({"S": filler(MAX_ITEM_BYTES + 1)})
}

#[tokio::test]
async fn an_item_a_request_carried_past_the_ceiling_is_the_item_sentence() {
    let router = seeded().await;

    // Rows: the surfaces that carry a whole ITEM. Each is told the ITEM is too big,
    // whichever of them carried it, and whether the sentence sits inside the envelope
    // follows this service's existing split for that surface's members.
    for (case, operation, request, expected) in [
        (
            "a single-item write",
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": {"pk": {"S": "big-1"}, "sk": {"S": "a"}, "body": oversized()}}),
            ITEM_TOO_BIG.to_string(),
        ),
        (
            "one request of a batched write",
            "BatchWriteItem",
            json!({"RequestItems": {TABLE: [{"PutRequest": {"Item":
                   {"pk": {"S": "big-2"}, "sk": {"S": "a"}, "body": oversized()}}}]}}),
            ITEM_TOO_BIG.to_string(),
        ),
        (
            "one action of a transaction",
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {"TableName": TABLE, "Item":
                   {"pk": {"S": "big-3"}, "sk": {"S": "a"}, "body": oversized()}}}]}),
            ITEM_TOO_BIG.to_string(),
        ),
        (
            "a statement that inserts one",
            "ExecuteStatement",
            json!({"Statement":
                   format!("INSERT INTO \"{TABLE}\" VALUE {{'pk': 'big-4', 'sk': 'a', \
                            'body': ?}}"),
                   "Parameters": [oversized()]}),
            ITEM_TOO_BIG.to_string(),
        ),
    ] {
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_change_that_would_grow_the_item_past_it_is_the_update_sentence() {
    let router = seeded().await;

    // Rows: the surfaces that state a CHANGE rather than a whole item, spanning the three
    // update actions that can grow an item (`SET`, `ADD` and a `list_append` concatenation)
    // and the three surfaces an update arrives through.
    for (case, operation, request) in [
        (
            "an update that sets an oversized value",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "SET body = :b",
                   "ExpressionAttributeValues": {":b": oversized()}}),
        ),
        (
            "an update that appends to a list",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "SET body = list_append(:b, :b)",
                   "ExpressionAttributeValues":
                       {":b": {"L": [{"S": filler(MAX_ITEM_BYTES / 2)}]}}}),
        ),
        (
            "an update that adds to a set",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "ADD tags :t",
                   "ExpressionAttributeValues":
                       {":t": {"SS": [filler(MAX_ITEM_BYTES + 1)]}}}),
        ),
        (
            "a statement that updates one",
            "ExecuteStatement",
            json!({"Statement":
                   format!("UPDATE \"{TABLE}\" SET body = ? WHERE pk = 'item-1' AND sk = 'a'"),
                   "Parameters": [oversized()]}),
        ),
        (
            // A VALUE an action CARRIES that no item could hold is a fact of the REQUEST,
            // settled while the element's own expression is read and before any table is
            // looked up, so it refuses the call — in the sentence the update path words that
            // quota with, because the value is one this change would store. The cases where
            // the item the change LEAVES is what crosses the quota are driven below.
            "one action of a transaction carrying a value no item could hold",
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": {"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "SET body = :b",
                   "ExpressionAttributeValues": {":b": oversized()}}}]}),
        ),
        (
            // The same class one byte finer: the value fits an item on its own, but the item
            // the statement ASKS FOR — that value beside the key it is written next to — does
            // not, and no stored item can make it fit, so this too is the request's own defect.
            "one action of a transaction asking for a value that only crosses beside its key",
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": {"TableName": TABLE, "Key": key(),
                   "UpdateExpression": "SET body = :b",
                   "ExpressionAttributeValues": {":b": {"S": filler(MAX_ITEM_BYTES)}}}}]}),
        ),
    ] {
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, UPDATE_TOO_BIG,
            "case {case:?} ({operation}): expected {UPDATE_TOO_BIG:?}, got {stated:?}"
        );
    }

    // The TRANSACTIONAL surface states the same sentence as the REASON the crossing action
    // reports where the bytes that cross the quota are the ones the STORED item already held —
    // "an item size becomes too large [...] because of changes made by the transaction", one
    // of the circumstances a cancellation documents — so the transaction is cancelled with
    // that action named.

    let near_the_ceiling = json!({"pk": {"S": "near-the-ceiling"}, "sk": {"S": "a"}});
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "near-the-ceiling"},
               "sk": {"S": "a"},
               "held": {"S": filler(MAX_ITEM_BYTES - 1024)}}}),
    )
    .await;
    for (case, request) in [(
        "a small value over an item already near the ceiling",
        json!({"TransactItems": [{"Update": {"TableName": TABLE,
               "Key": near_the_ceiling.clone(),
               "UpdateExpression": "SET body = :b",
               "ExpressionAttributeValues": {":b": {"S": filler(2048)}}}}]}),
    )] {
        let (status, answered) = call(&router, "TransactWriteItems", request).await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message(&answered).as_str()
            ),
            (
                400,
                "TransactionCanceledException",
                "Transaction cancelled, please refer cancellation reasons for specific reasons \
                 [ValidationError]"
            ),
            "case {case:?}: expected the transaction cancelled over that action, got {status} \
             with {answered}"
        );
        assert_eq!(
            (
                answered["CancellationReasons"][0]["Code"].as_str(),
                answered["CancellationReasons"][0]["Message"].as_str(),
            ),
            (Some("ValidationError"), Some(UPDATE_TOO_BIG)),
            "case {case:?}: expected that action's reason to word the ceiling, got {answered}"
        );
    }

    // The seeded item is where it was: a change that did not go through writes nothing.
    let read = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        read["Item"]["s"],
        json!({"S": "beta"}),
        "case \"the changes that did not go through wrote nothing\": expected the stored item, \
         got {read}"
    );
    assert_eq!(
        read["Item"].get("body"),
        None,
        "case \"the changes that did not go through wrote nothing\": no oversized value landed, \
         got {read}"
    );
}

#[tokio::test]
async fn a_change_inside_the_ceiling_is_applied_and_reads_back() {
    let router = seeded().await;

    // The CONTROL for rule 2: the same update shapes, with values the ceiling admits, are
    // applied — so the refusals above are the ceiling and not a blanket rejection.
    ok(
        &router,
        "UpdateItem",
        json!({"TableName": TABLE, "Key": key(),
               "UpdateExpression": "SET body = :b ADD tags :t",
               "ExpressionAttributeValues": {":b": {"S": filler(1024)},
                                             ":t": {"SS": ["red"]}}}),
    )
    .await;
    let read = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        read["Item"]["body"],
        json!({"S": filler(1024)}),
        "case \"an update inside the ceiling\": expected the written value, got {read}"
    );
    assert_eq!(
        read["Item"]["tags"],
        json!({"SS": ["red"]}),
        "case \"an update inside the ceiling\": expected the added member, got {read}"
    );
}

// ===========================================================================
// RULE 3. A transaction's actions add up to at most 4 MB
// ===========================================================================

/// The documented aggregate ceiling of one transaction, 4 MB ("Service, Account, and Table
/// Quotas": "The aggregate size of the items in the transaction cannot exceed 4 MB").
const MAX_TRANSACTION_BYTES: usize = 4 * 1024 * 1024;

/// One `Put` of an item carrying `bytes` of filler under the key `pk`.
fn put_of(pk: &str, bytes: usize) -> Value {
    json!({"Put": {"TableName": TABLE,
                   "Item": {"pk": {"S": pk}, "sk": {"S": "a"},
                            "body": {"S": filler(bytes)}}}})
}

#[tokio::test]
async fn a_transaction_past_the_aggregate_ceiling_is_refused_whole() {
    let router = seeded().await;

    // Each action is well inside the ITEM ceiling; it is their TOTAL that crosses the
    // transaction's. The rows differ along how the total is reached — many middling
    // actions, and fewer larger ones — because the ceiling is on the sum and not on any
    // one action.
    for (case, actions, each) in [
        ("sixteen actions of 300 KB", 16, 300 * 1024),
        ("eleven actions of 390 KB", 11, 390 * 1024),
    ] {
        let items: Vec<Value> = (0..actions)
            .map(|at| put_of(&format!("agg-{case}-{at}"), each))
            .collect();
        assert!(
            actions * each > MAX_TRANSACTION_BYTES,
            "case {case:?}: the row must state a total past the ceiling, got {}",
            actions * each
        );
        let stated = refusal(
            &router,
            case,
            "TransactWriteItems",
            json!({"TransactItems": items}),
        )
        .await;
        // The CONTRACT is the class: a top-level `ValidationException` refusing the whole
        // request (asserted by `refusal`), stated as its own sentence and NOT as either
        // item-ceiling sentence — those speak about one item, and no item of this
        // transaction crossed one.
        assert!(
            stated != ITEM_TOO_BIG && stated != UPDATE_TOO_BIG && !stated.is_empty(),
            "case {case:?}: the aggregate ceiling is its own refusal, not an item's, got \
             {stated:?}"
        );
        // Nothing of a refused transaction landed.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE,
                   "Key": {"pk": {"S": format!("agg-{case}-0")}, "sk": {"S": "a"}}}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case {case:?}: a refused transaction writes nothing, got {read}"
        );
    }
}

#[tokio::test]
async fn a_transaction_inside_the_aggregate_ceiling_is_applied() {
    let router = seeded().await;

    // The CONTROL for rule 3: the same shape, adding up UNDER the ceiling (ten actions of
    // 300 KB is a little over 3 MB), is served and every action of it is readable
    // afterwards.
    let items: Vec<Value> = (0..10)
        .map(|at| put_of(&format!("under-{at}"), 300 * 1024))
        .collect();
    ok(&router, "TransactWriteItems", json!({"TransactItems": items})).await;
    for at in [0usize, 9] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE,
                   "Key": {"pk": {"S": format!("under-{at}")}, "sk": {"S": "a"}}}),
        )
        .await;
        assert_eq!(
            read["Item"]["pk"],
            json!({"S": format!("under-{at}")}),
            "case \"ten actions of 300 KB\": action {at} must have landed, got {read}"
        );
    }
}
