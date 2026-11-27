// SPDX-License-Identifier: CC0-1.0

//! Where an UPDATE's item-size ceiling sits once the key portion's floor is accounted for:
//! the same ceiling for every item whose primary key holds less than the floor.
//!
//! The rule under test: what an update's resulting item is charged against the documented
//! 400 KB item ceiling is the bytes of the attributes the change addresses plus what the part
//! of the item it leaves alone — here the primary key, which is all these items hold besides
//! the written attribute — is charged, and that part is charged at a MINIMUM —
//! `state::UNTOUCHED_PORTION_FLOOR_BYTES` — however few bytes it actually holds. Two
//! consequences, both asserted here over measured boundaries rather than computed ones:
//!
//! * a key portion below that floor buys no headroom, so the largest change two items accept
//!   is the SAME when their keys differ only in how many bytes they hold below the floor —
//!   which is what a client probing its own payload budget on one item and writing the next
//!   relies on; and
//! * a key portion at or above the floor fills it and is charged the bytes it holds, so the
//!   item tops out at the quota itself there and the ceiling shrinks byte for byte with the
//!   key past the floor.
//!
//! The rows span what the rule generalises over: key portions two, nine and ten bytes long
//! (the last pair differing by the single byte that must move nothing), one landing exactly on
//! the floor and one well past it. Each row's ceiling is MEASURED — the suite walks candidate
//! operand lengths down from the largest an item could hold if the key were charged its own
//! bytes, and takes the first one the service admits — so an assertion here is about the
//! service's boundary and not about this suite's arithmetic.
//!
//! `TransactWriteItems`' `Update` action is bound by the same ceiling as the standalone
//! `UpdateItem`, and states the crossing as the CALL's own refusal where the key portion sits
//! below the floor — the request was then held to a ceiling of its own — and as that action's
//! cancellation reason where the portion fills the floor and the ceiling is the quota itself,
//! so the measured boundary is re-asserted through it.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB
//! Developer Guide ("The maximum item size in DynamoDB is 400 KB"). The floor under the
//! untouched part of an item is measured behavior, stated once by the service
//! (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).
//! `update_item_size_floor.rs` crosses the same ceiling with every update surface; this suite
//! is about where the ceiling SITS as the key portion varies.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The documented item ceiling, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// The least an item's primary-key portion is charged against that ceiling on the update
/// path, where it is all the change leaves untouched, as the service states it
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).
const KEY_PORTION_FLOOR: usize = 22;

/// AWS's fixed sentence for a change that would grow the stored item past the ceiling.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The attribute every row's change writes, one byte of name.
const WRITTEN: &str = "v";

/// One table shape: how many bytes its key portion holds, and the values it is keyed by.
struct KeyShape {
    case: &'static str,
    table: &'static str,
    partition: &'static str,
    partition_value: &'static str,
}

impl KeyShape {
    /// The bytes this shape's key portion stores: the key attribute's name plus its value.
    fn key_bytes(&self) -> usize {
        self.partition.len() + self.partition_value.len()
    }

    /// What this shape's key portion is CHARGED: its own bytes once it fills the floor, and
    /// the floor while it does not.
    fn key_charge(&self) -> usize {
        self.key_bytes().max(KEY_PORTION_FLOOR)
    }

    fn key(&self) -> Value {
        json!({self.partition: {"S": self.partition_value}})
    }
}

/// Key portions below the floor — including two that differ by a single byte — one landing
/// exactly on it, and one well past it.
const SHAPES: &[KeyShape] = &[
    KeyShape {
        case: "a two-byte key portion",
        table: "KeyCeilingTwo",
        partition: "k",
        partition_value: "1",
    },
    KeyShape {
        case: "a nine-byte key portion",
        table: "KeyCeilingNine",
        partition: "pk",
        partition_value: "1234567",
    },
    KeyShape {
        case: "a ten-byte key portion, one byte more than the nine-byte one",
        table: "KeyCeilingTen",
        partition: "pk",
        partition_value: "12345678",
    },
    KeyShape {
        case: "a key portion landing exactly on the floor",
        table: "KeyCeilingAtTheFloor",
        partition: "pk",
        partition_value: "12345678901234567890",
    },
    KeyShape {
        case: "a key portion well past the floor",
        table: "KeyCeilingPastTheFloor",
        partition: "partitionkeyname",
        partition_value: "0123456789012345",
    },
];

/// RULE: the largest change an item accepts leaves its non-key bytes at the quota less what
/// its key portion is CHARGED — so every key below the floor shares one ceiling, and a key
/// that fills the floor is charged its own bytes.
#[tokio::test]
async fn the_update_ceiling_is_the_quota_less_what_the_key_portion_is_charged() {
    let router = emulator();
    let mut non_key_ceilings: Vec<(&str, usize)> = Vec::new();

    for shape in SHAPES {
        create_active_table(&router, shape.table, &[(shape.partition, "HASH", "S")]).await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": shape.table, "Item": shape.key()}),
        )
        .await;
        let largest = largest_accepted_operand(&router, shape).await;
        let non_key = largest + WRITTEN.len();
        assert_eq!(
            non_key + shape.key_charge(),
            MAX_ITEM_BYTES,
            "case {:?}: expected the {non_key} non-key byte(s) admitted plus the {} byte(s) its \
             {}-byte key portion is charged to reach the quota",
            shape.case,
            shape.key_charge(),
            shape.key_bytes(),
        );
        non_key_ceilings.push((shape.case, non_key));
    }

    // Every key portion below the floor, and the one landing exactly on it, share ONE ceiling
    // over the item's non-key bytes: the defect this pins is a ceiling that moved with the
    // key's own length.
    let below: Vec<(&str, usize)> = SHAPES
        .iter()
        .zip(&non_key_ceilings)
        .filter(|(shape, _)| shape.key_bytes() <= KEY_PORTION_FLOOR)
        .map(|(_, measured)| *measured)
        .collect();
    let (first_case, first) = below[0];
    for (case, measured) in &below {
        assert_eq!(
            *measured, first,
            "case {case:?}: a key portion at or below the floor must share the ceiling \
             {first} measured for {first_case:?}, got {measured}"
        );
    }

    // A key portion PAST the floor fills it and is charged its own bytes, so its ceiling is
    // lower by exactly what it holds past the floor.
    for (shape, (case, measured)) in SHAPES.iter().zip(&non_key_ceilings) {
        if shape.key_bytes() > KEY_PORTION_FLOOR {
            let expected = first - (shape.key_bytes() - KEY_PORTION_FLOOR);
            assert_eq!(
                *measured, expected,
                "case {case:?}: a key portion {} byte(s) past the floor must lower the ceiling \
                 by as much, expected {expected}, got {measured}",
                shape.key_bytes() - KEY_PORTION_FLOOR
            );
        }
    }
}

/// RULE: `TransactWriteItems`' `Update` is bound by the standalone `UpdateItem`'s ceiling —
/// the ceiling is the ITEM's and does not move with the surface the change arrived through —
/// and WHO answers for a crossing follows which ceiling the update path held the request to:
/// where the key portion sits BELOW the floor it is charged at, the request was held to a
/// ceiling of its own, below the quota and the same however the store looks, so the CALL is
/// refused in the class and the sentence the single-item path states; where the key portion
/// fills that floor the ceiling is the quota's own count, so a crossing is the item having
/// become too large and the transaction is CANCELLED with that action's reason. The byte the
/// change stops at is the one `UpdateItem` stops at either way, and nothing is applied.
#[tokio::test]
async fn a_transactional_update_shares_the_single_item_ceiling() {
    let router = emulator();
    for shape in SHAPES {
        let table = format!("Txn{}", shape.table);
        create_active_table(&router, &table, &[(shape.partition, "HASH", "S")]).await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": table, "Item": shape.key()}),
        )
        .await;
        let largest = MAX_ITEM_BYTES - shape.key_charge() - WRITTEN.len();

        let (status, body) = call(&router, "TransactWriteItems", transact(&table, shape, largest))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the transactional update at the ceiling to be applied, got \
             {status} with {}",
            shape.case,
            message(&body)
        );

        let (status, body) = call(
            &router,
            "TransactWriteItems",
            transact(&table, shape, largest + 1),
        )
        .await;
        if shape.key_bytes() < KEY_PORTION_FLOOR {
            // The item this action would leave is written by the statement itself over a key
            // portion the update path charges at its floor, so this request was held to a
            // ceiling of its own: the CALL is refused with the single-item path's sentence and
            // no action is graded for it.
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, "ValidationException", UPDATE_TOO_BIG),
                "case {:?}: expected the call refused one byte past the charged ceiling, got \
                 {status} with {body}",
                shape.case
            );
            assert!(
                body.get("CancellationReasons").is_none(),
                "case {:?}: a refused call grades no action, so it reports no reason list, got \
                 {body}",
                shape.case
            );
        } else {
            // The key portion fills that floor, so the ceiling this request was held to is the
            // quota's own count and one byte past it is an item that became too large: THIS
            // action's failure, which cancels the transaction.
            assert_eq!(
                (
                    status,
                    error_code(&body).as_str(),
                    body["CancellationReasons"][0]["Code"].as_str(),
                    body["CancellationReasons"][0]["Message"].as_str(),
                ),
                (
                    400,
                    "TransactionCanceledException",
                    Some("ValidationError"),
                    Some(UPDATE_TOO_BIG),
                ),
                "case {:?}: expected that action's cancellation one byte past the quota, got \
                 {status} with {body}",
                shape.case
            );
        }
        let stored = stored_length(&router, &table, shape).await;
        assert_eq!(
            stored,
            Some(largest),
            "case {:?}: a refused or cancelled transactional update must leave the stored item \
             where it was, got {stored:?} byte(s)",
            shape.case
        );
    }
}

/// The `TransactWriteItems` request that writes an operand of `operand` single-byte characters
/// into this shape's item.
fn transact(table: &str, shape: &KeyShape, operand: usize) -> Value {
    json!({"TransactItems": [{"Update": {
        "TableName": table,
        "Key": shape.key(),
        "UpdateExpression": "SET v = :v",
        "ExpressionAttributeValues": {":v": {"S": "x".repeat(operand)}},
    }}]})
}

/// The largest `SET v = :v` operand this shape's item accepts, MEASURED: candidate lengths are
/// walked down from the largest the item could hold if its key portion were charged only the
/// bytes it holds, and the first one the service admits is the boundary. One byte past it is
/// asserted to be refused with the update path's sentence, so the boundary is pinned from both
/// sides.
///
/// The walk is bounded by the floor itself: the key portion is charged at most the floor more
/// than it holds, so no boundary can lie further down than that.
async fn largest_accepted_operand(
    router: &smithy_server_runtime::MultiServiceRouter,
    shape: &KeyShape,
) -> usize {
    let flat = MAX_ITEM_BYTES - shape.key_bytes() - WRITTEN.len();
    let floor = flat.saturating_sub(KEY_PORTION_FLOOR + 1);
    let mut refused_at: Option<usize> = None;
    for candidate in (floor..=flat).rev() {
        let (status, body) = call(router, "UpdateItem", update(shape, candidate)).await;
        if status == 200 {
            if let Some(refused) = refused_at {
                assert_eq!(
                    refused,
                    candidate + 1,
                    "case {:?}: expected the refusal to start one byte past the largest \
                     accepted operand {candidate}, got {refused}",
                    shape.case
                );
            }
            return candidate;
        }
        assert_eq!(
            (status, message(&body).as_str()),
            (400, UPDATE_TOO_BIG),
            "case {:?}: expected a {candidate}-byte operand to be either applied or refused \
             with the update path's ceiling sentence, got {status} with {body}",
            shape.case
        );
        refused_at = Some(candidate);
    }
    panic!(
        "case {:?}: no operand within {} byte(s) of the flat ceiling was accepted",
        shape.case,
        KEY_PORTION_FLOOR + 1
    );
}

/// The `UpdateItem` request that writes an operand of `operand` single-byte characters into
/// this shape's item.
fn update(shape: &KeyShape, operand: usize) -> Value {
    json!({
        "TableName": shape.table,
        "Key": shape.key(),
        "UpdateExpression": "SET v = :v",
        "ExpressionAttributeValues": {":v": {"S": "x".repeat(operand)}},
    })
}

/// The length of the written attribute as a `ConsistentRead` `GetItem` reports it.
async fn stored_length(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    shape: &KeyShape,
) -> Option<usize> {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": shape.key(), "ConsistentRead": true}),
    )
    .await;
    read["Item"][WRITTEN]["S"].as_str().map(str::len)
}
