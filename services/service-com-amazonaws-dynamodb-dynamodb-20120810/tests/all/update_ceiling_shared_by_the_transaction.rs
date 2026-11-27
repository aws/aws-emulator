// SPDX-License-Identifier: CC0-1.0

//! What a transaction APPLIES when its `Update` crosses the charged update ceiling while the item
//! that change would leave still FITS the documented item quota — the crossing the request's own
//! accounting makes, which refuses the CALL rather than cancelling the transaction.
//!
//! The rule under test: an update is weighed against the 400 KB item quota over what the item it
//! leaves is CHARGED — the bytes it stores, with the part of it the change does not address
//! charged at a minimum floor (`state::UNTOUCHED_PORTION_FLOOR_BYTES`) where the change is stated
//! as an expression, so the
//! transaction's `Update` tops out exactly where the single-item `UpdateItem` of the same
//! expression does. Where the item itself fits the quota and only that charged ceiling is crossed,
//! nothing became too large: the request asks for more than the update path admits, so the CALL is
//! refused with the service-common `ValidationException` and the update path's fixed sentence —
//! and a refused call applies NOTHING, so every action the transaction carried is discarded, the
//! sibling actions beside the oversized `Update` included. The same transaction with that `Update`
//! at its boundary commits every one of them.
//!
//! The rows span what the rule generalises over: the KIND of sibling action standing beside the
//! oversized one, each with an effect a later read can see (a `Put` that must not appear, a
//! `Delete` that must not remove, an `Update` of another item that must not change it), crossed
//! with WHERE that sibling sits relative to the oversized action — and each row driven twice, once
//! with the `Update` one byte past its boundary and once at it, so what the refusal answers is the
//! ceiling and not the shape of the call.
//!
//! `transactional_update_ceiling_class.rs` drives the other half of the same rule — the item that
//! really did become too large, which is the documented CANCELLATION circumstance and discards its
//! siblings through the reason list — and `update_item_ceiling_key_portion.rs` and
//! `update_item_size_floor.rs` drive where the floor-bound boundary SITS on each surface. This
//! suite is about what a CALL refused for that ceiling leaves behind.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB Developer
//! Guide ("The maximum item size in DynamoDB is 400 KB"). `TransactWriteItems` declares no size
//! error of its own (smithy_query query="operation" operation="TransactWriteItems"), so a refusal
//! for it is the service-common `ValidationException`.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::UNTOUCHED_PORTION_FLOOR_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row writes into: one string partition key, one byte of name.
const TABLE: &str = "TxnCallRefusedCeiling";

/// The partition key attribute's name.
const KEY: &str = "k";

/// The documented item quota, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// AWS's fixed sentence for a change that would leave the item past that quota. The WHOLE
/// message: it carries no value from the request or from the stored item.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The class a size refusal is stated in, no modelled error of either operation covering one.
const VALIDATION: &str = "ValidationException";

/// The attribute the oversized change writes, one byte of name.
const WRITTEN: &str = "v";

/// The value every item is seeded with, and the value a discarded action must leave behind.
const SEEDED: &str = "start";

/// The value a sibling action writes, so a landed sibling is told from a discarded one.
const BY_THE_SIBLING: &str = "sibling";

/// One kind of sibling action standing beside the oversized `Update`, each with an effect a later
/// read can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sibling {
    /// A `Put` of an item whose key holds nothing yet: landed means the item is there.
    Put,
    /// A `Delete` of an item that is there: landed means the item is gone.
    Delete,
    /// An `Update` of ANOTHER item, well inside the ceiling: landed means its value changed.
    Update,
}

const SIBLINGS: &[Sibling] = &[Sibling::Put, Sibling::Delete, Sibling::Update];

/// Where the sibling sits in the `TransactItems` list relative to the oversized action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Before,
    After,
}

const SLOTS: &[Slot] = &[Slot::Before, Slot::After];

impl Sibling {
    fn case(self) -> &'static str {
        match self {
            Sibling::Put => "a Put of an item whose key holds nothing",
            Sibling::Delete => "a Delete of an item that is there",
            Sibling::Update => "an Update of another item, well inside the ceiling",
        }
    }

    /// Put the item this sibling acts on into the state its row starts from: the two actions that
    /// act on an EXISTING item need one, and the `Put` needs its key to hold nothing.
    async fn seed(self, router: &MultiServiceRouter, key: &str) {
        if self == Sibling::Put {
            return;
        }
        ok(
            router,
            "PutItem",
            json!({"TableName": TABLE, "Item": {KEY: {"S": key}, WRITTEN: {"S": SEEDED}}}),
        )
        .await;
    }

    /// This sibling's `TransactItems` element.
    fn action(self, key: &str) -> Value {
        match self {
            Sibling::Put => json!({"Put": {
                "TableName": TABLE,
                "Item": {KEY: {"S": key}, WRITTEN: {"S": BY_THE_SIBLING}},
            }}),
            Sibling::Delete => json!({"Delete": {"TableName": TABLE, "Key": {KEY: {"S": key}}}}),
            Sibling::Update => json!({"Update": {
                "TableName": TABLE,
                "Key": {KEY: {"S": key}},
                "UpdateExpression": "SET v = :v",
                "ExpressionAttributeValues": {":v": {"S": BY_THE_SIBLING}},
            }}),
        }
    }

    /// What a read of this sibling's item must report while its action has NOT been applied: the
    /// state the row seeded, which for the `Put` is no item at all.
    fn before_it_lands(self) -> Option<String> {
        match self {
            Sibling::Put => None,
            Sibling::Delete | Sibling::Update => Some(SEEDED.to_string()),
        }
    }

    /// What a read of it must report once the action HAS been applied.
    fn once_it_lands(self) -> Option<String> {
        match self {
            Sibling::Put | Sibling::Update => Some(BY_THE_SIBLING.to_string()),
            Sibling::Delete => None,
        }
    }
}

impl Slot {
    fn case(self) -> &'static str {
        match self {
            Slot::Before => "standing before it",
            Slot::After => "standing after it",
        }
    }

    /// The `TransactItems` list for this slot.
    fn actions(self, sibling: Value, oversized: Value) -> Value {
        match self {
            Slot::Before => json!([sibling, oversized]),
            Slot::After => json!([oversized, sibling]),
        }
    }
}

/// RULE: a transaction whose `Update` crosses the charged ceiling while the item it would leave
/// fits the quota is refused as the CALL it is — 400, the validation class, the update path's
/// sentence — and applies nothing, whatever stands beside it and wherever that stands; the same
/// transaction with the change at its boundary applies every action.
#[tokio::test]
async fn a_call_refused_for_the_update_ceiling_applies_none_of_its_actions() {
    let router = emulator();
    create_active_table(&router, TABLE, &[(KEY, "HASH", "S")]).await;

    // The part of every item here the change leaves alone holds three bytes — the one-byte key
    // attribute's name beside a two-byte value — fewer than the floor, so it is charged that
    // floor and the boundary sits under the quota by what it does not fill.
    let key_bytes = KEY.len() + 2;
    let floor =
        usize::try_from(UNTOUCHED_PORTION_FLOOR_BYTES).expect("the untouched-portion floor is a count");
    let charged_key = key_bytes.max(floor);
    // At the boundary what the item the change leaves is CHARGED is exactly the quota; one byte
    // more crosses it while the bytes that item STORES are still inside the quota, which is the
    // crossing this suite is about.
    let at_the_boundary = MAX_ITEM_BYTES - charged_key - WRITTEN.len();

    for (row, sibling) in SIBLINGS.iter().enumerate() {
        for (column, slot) in SLOTS.iter().enumerate() {
            let (sibling, slot) = (*sibling, *slot);
            // Each row addresses its own pair of items, so no row reads what another wrote. Both
            // key values hold two bytes, which is the key portion the boundary above was computed
            // for.
            let seat = row * SLOTS.len() + column;
            let updated = format!("u{seat}");
            let touched = format!("s{seat}");
            ok(
                &router,
                "PutItem",
                json!({"TableName": TABLE,
                       "Item": {KEY: {"S": &updated}, WRITTEN: {"S": SEEDED}}}),
            )
            .await;
            sibling.seed(&router, &touched).await;

            let oversized = |operand: usize| {
                json!({"Update": {
                    "TableName": TABLE,
                    "Key": {KEY: {"S": &updated}},
                    "UpdateExpression": "SET v = :v",
                    "ExpressionAttributeValues": {":v": {"S": "x".repeat(operand)}},
                }})
            };
            let case = format!("{} {}", sibling.case(), slot.case());

            // One byte past the boundary: the CALL is refused and nothing is applied.
            let (status, body) = call(
                &router,
                "TransactWriteItems",
                json!({"TransactItems": slot.actions(
                    sibling.action(&touched),
                    oversized(at_the_boundary + 1),
                )}),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, VALIDATION, UPDATE_TOO_BIG),
                "case {case:?}: expected the CALL refused for the charged ceiling over a \
                 {key_bytes}-byte key charged as {charged_key}, got {status} with {body}"
            );
            let held = stored_value(&router, &updated).await;
            assert_eq!(
                held.as_deref(),
                Some(SEEDED),
                "case {case:?}: the refused update must leave its own item where it was, got \
                 {held:?}"
            );
            let beside = stored_value(&router, &touched).await;
            assert_eq!(
                beside,
                sibling.before_it_lands(),
                "case {case:?}: a refused call applies none of its actions, so the sibling must \
                 not have landed, got {beside:?}"
            );

            // AT the boundary the very same transaction commits, the sibling with it.
            let (status, body) = call(
                &router,
                "TransactWriteItems",
                json!({"TransactItems": slot.actions(
                    sibling.action(&touched),
                    oversized(at_the_boundary),
                )}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the change at its boundary commits with its sibling, got {status} \
                 with {}",
                message(&body)
            );
            let held = stored_value(&router, &updated).await;
            assert_eq!(
                held.map(|value| value.len()),
                Some(at_the_boundary),
                "case {case:?}: expected the whole {at_the_boundary}-byte operand to be stored at \
                 the boundary"
            );
            let beside = stored_value(&router, &touched).await;
            assert_eq!(
                beside,
                sibling.once_it_lands(),
                "case {case:?}: expected the committed sibling's effect to read back, got \
                 {beside:?}"
            );
        }
    }
}

/// The written attribute of `key`'s item as a `ConsistentRead` `GetItem` reports it, and `None`
/// where the key holds no item.
async fn stored_value(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY: {"S": key}}, "ConsistentRead": true}),
    )
    .await;
    read["Item"][WRITTEN]["S"].as_str().map(ToString::to_string)
}
