// SPDX-License-Identifier: CC0-1.0

//! WHOSE failure a crossing of the item-size quota is, for a transactional `Update` whose
//! CLAUSES are what push it over the ceiling.
//!
//! RULE: what an update is weighed against is not the 400 KB quota alone — on top of the bytes
//! the item stores it adds what the REQUEST's own shape costs, the frame each clause of the
//! expression carries among them. Whose failure a crossing of that charged ceiling is follows
//! the bytes the STATEMENT asks for: the item these rows leave is written entirely by the
//! statement's own clauses over the key it addresses, so the statement itself asks for more
//! than an update may land — a fact of the REQUEST, which no stored item can satisfy — and the
//! CALL is refused on either side of the quota, in the class and the sentence the single-item
//! `UpdateItem` of the very same expression is refused with. A transaction is CANCELLED where
//! the crossing comes from bytes the stored item ALREADY held, which is
//! `transactional_update_ceiling_class.rs`'s boundary.
//!
//! The two rows below sit ON the quota, one byte apart, with everything else — the table,
//! the key portion, the expression, the number of clauses — identical: the only difference is
//! whether the item the change leaves holds 409,600 bytes or one more. Each is driven through
//! both surfaces, so the byte a transaction stops at is shown to be the byte `UpdateItem` stops
//! at, and a control inside the ceiling commits and reads back.
//!
//! `update_item_ceiling_key_portion.rs` drives the same boundary over the other charge an
//! update carries, the key portion's floor; this suite is about the clauses' own.
//!
//! Source of the quota: "Service, Account, and Table Quotas" in the Amazon DynamoDB Developer
//! Guide ("The maximum item size in DynamoDB is 400 KB"). Source of the cancel circumstance:
//! `TransactWriteItems`' `TransactionCanceledException`
//! (`smithy_query(query="shape",shape="TransactionCanceledException")`).

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The documented item quota, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// AWS's fixed sentence for a change that would leave the item past the quota.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The table every row addresses. Its key portion holds exactly the floor the update path
/// charges the untouched part of an item at (`state::UNTOUCHED_PORTION_FLOOR_BYTES`, 22 bytes:
/// a two-byte name and a twenty-byte value), so the key is charged the bytes it holds and the
/// only charge left over the item's own bytes is the one the CLAUSES carry.
const TABLE: &str = "TxnClauseChargeCeiling";
const PARTITION: &str = "pk";
const PARTITION_VALUE: &str = "12345678901234567890";

/// The two attributes the rows' two clauses write, one byte of name each.
const FIRST: &str = "a";
const SECOND: &str = "b";

/// The bytes the key portion of every row's item holds.
fn key_bytes() -> usize {
    PARTITION.len() + PARTITION_VALUE.len()
}

fn key() -> Value {
    json!({PARTITION: {"S": PARTITION_VALUE}})
}

/// The two-clause update that leaves an item of `stored` bytes: the key portion, the two
/// attribute names, and the two values — the first one byte long, the second whatever is left.
fn two_clause_update(stored: usize) -> Value {
    let fixed = key_bytes() + FIRST.len() + SECOND.len() + 1;
    let second = stored - fixed;
    json!({
        "TableName": TABLE,
        "Key": key(),
        "UpdateExpression": format!("SET {FIRST} = :a, {SECOND} = :b"),
        "ExpressionAttributeValues": {
            ":a": {"S": "x"},
            ":b": {"S": "y".repeat(second)},
        },
    })
}

/// RULE: both sides of the quota are the STATEMENT's own ask here — its clauses write the whole
/// item over the key it addresses — so both refuse the CALL, through the transaction exactly as
/// through the single-item surface, and neither grades an action.
#[tokio::test]
async fn what_the_statement_asks_for_decides_who_answers_for_the_ceiling() {
    let router = emulator();
    create_active_table(&router, TABLE, &[(PARTITION, "HASH", "S")]).await;

    // A control first: a change comfortably inside the ceiling is applied through the
    // transaction and reads back, so what follows is the ceiling and not a blanket refusal.
    let inside = MAX_ITEM_BYTES - 1024;
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": two_clause_update(inside)}]}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
    )
    .await;
    let stored = read["Item"][SECOND]["S"].as_str().map(str::len);
    let expected = Some(inside - key_bytes() - FIRST.len() - SECOND.len() - 1);
    assert_eq!(
        stored, expected,
        "case \"a change inside the ceiling\": expected the whole operand stored, expected \
         {expected:?} byte(s), got {stored:?}"
    );

    // (case, the bytes the item the change leaves holds)
    let rows: [(&str, usize); 2] = [
        (
            "an item that fills the quota exactly, over the ceiling only by what its clauses \
             cost",
            MAX_ITEM_BYTES,
        ),
        ("an item one byte past the quota", MAX_ITEM_BYTES + 1),
    ];

    for (case, stored_bytes) in rows {
        // The single-item surface refuses both: the ceiling it stops at is the charged one,
        // and the sentence is the update path's.
        let (status, body) = call(&router, "UpdateItem", two_clause_update(stored_bytes)).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", UPDATE_TOO_BIG),
            "case {case:?}: expected UpdateItem to refuse the change, got {status} with {body}"
        );

        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": two_clause_update(stored_bytes)}]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", UPDATE_TOO_BIG),
            "case {case:?}: expected the call refused, got {status} with {body}"
        );
        assert!(
            body.get("CancellationReasons").is_none(),
            "case {case:?}: a refused call grades no action, so it reports no reason list, \
             got {body}"
        );

        // Neither answer wrote anything: the item is still the control's.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
        )
        .await;
        let stored = read["Item"][SECOND]["S"].as_str().map(str::len);
        assert_eq!(
            stored, expected,
            "case {case:?}: a refused or cancelled change leaves the stored item where it was, \
             expected {expected:?} byte(s), got {stored:?}"
        );
    }
}
