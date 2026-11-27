// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for WHO answers for the documented item-size quota when a
//! `TransactWriteItems` write crosses it, driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<Operation>`).
//!
//! RULE: an update is weighed over the bytes the item it leaves STORES plus what the update
//! path charges the REQUEST for its own shape — the part of the item its clauses leave alone
//! charged at a floor however few bytes it holds, and the frame each further clause states —
//! and WHO answers for a crossing follows which of those two ceilings bound the request. A
//! request charged nothing of its own was held to the quota's own count, so the only thing a
//! crossing can be is the item having become too large: that is "an item size becomes too
//! large (larger than 400 KB) [...] or a similar validation error occurs because of changes
//! made by the transaction", one of the circumstances `TransactionCanceledException`
//! documents, so the transaction is CANCELLED with that action named in the ordered reason
//! list and nothing is applied. A request charged something of its own was held to a ceiling
//! UNDER the quota that no stored item can lift, so crossing it is a fact of the REQUEST and
//! the CALL is refused, exactly as the single-item `UpdateItem` of the same statement is. An
//! item a `Put` CARRIES is the request's own either way, weighed in the bytes it carries.
//!
//! The rows span what the rule generalises over: how much the stored item holds beside the
//! change (a five-byte attribute, a 100 KB one, a second attribute the change never names),
//! what the update path charges the request for (a key portion below its floor, a second
//! clause), and the surface the statement arrives through. The BOUNDARY is driven from both
//! sides for every leg: the item that fills the ceiling exactly commits, or is stored and read
//! back whole, and the one byte past it is cancelled or refused.
//!
//! Source of the quota: "Service, Account, and Table Quotas" in the Amazon DynamoDB Developer
//! Guide ("The maximum item size in DynamoDB is 400 KB"), counted the way the guide counts an
//! item: "the lengths of its attribute names and values". Source of the cancel circumstance:
//! `TransactWriteItems`' own `TransactionCanceledException`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, error_type, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: one string partition key.
const TABLE: &str = "TxnQuotaOverStoredBytes";

/// The documented item quota, 400 KB, in the bytes an item stores.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// The partition key's attribute name, and a key VALUE long enough that the part of the item
/// a change leaves alone fills the floor the update path charges it at
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`, 22 bytes), so the ceiling these rows drive is the
/// quota itself.
const PARTITION: &str = "pk";
const KEY_FILL: usize = 20;

/// The attribute the stored item carries and no change below names — a name that is no
/// reserved word of the expression language, so the row that REPLACES it can address it
/// directly.
const HELD: &str = "held";

/// The attribute every change writes.
const GROWN: &str = "grow";

/// A second attribute one row's stored item carries, likewise never named by a change, and
/// the attribute the two-clause change writes.
const KEPT: &str = "keep";

/// The key tag the two-clause change addresses, which holds no item: what such a change leaves
/// alone is its key portion, which fills the floor, so the only thing the update path charges
/// that request for is the second clause's own frame.
const SECOND_CLAUSE_TAG: &str = "two-clause";

/// The table whose key portion holds the FEWEST bytes an item can — one byte of name beside
/// one byte of value — so the part of the item a change leaves alone sits below the floor it is
/// charged at and the request is held to a ceiling of its own.
const SHORT_TABLE: &str = "TxnQuotaShortKey";
const SHORT_PARTITION: &str = "k";
const SHORT_KEY_VALUE: &str = "1";

/// The least the part of an item a change leaves alone is charged against the quota, as the
/// service states it (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).
const UNTOUCHED_PORTION_FLOOR: usize = 22;

/// AWS's fixed sentence for a change that would leave the item past the quota.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// AWS's fixed sentence for a request that CARRIED an item past the quota.
const ITEM_TOO_BIG: &str = "Item size has exceeded the maximum allowed size";

/// The whole shape id the wire carries for the validation class, which is what a client
/// branching on `__type` matches.
const VALIDATION_SHAPE_ID: &str = "com.amazon.coral.validate#ValidationException";

/// The `CancellationReason$Code` an action that broke a rule of the service reports.
const VALIDATION_ERROR_CODE: &str = "ValidationError";

/// The whole top-level sentence a cancelled transaction carries over one offending slot.
const CANCELLED_MESSAGE: &str = "Transaction cancelled, please refer cancellation reasons for \
                                 specific reasons [ValidationError]";

async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[(PARTITION, "HASH", "S")]).await;
    router
}

/// The key value a row addresses: its own tag, padded to [`KEY_FILL`] bytes so every row's key
/// portion holds the same number of bytes.
fn key_value(tag: &str) -> String {
    let mut value = tag.to_string();
    assert!(
        value.len() <= KEY_FILL,
        "case {tag:?}: the row tag must fit the key value"
    );
    while value.len() < KEY_FILL {
        value.push('0');
    }
    value
}

fn key_of(tag: &str) -> Value {
    json!({PARTITION: {"S": key_value(tag)}})
}

/// The bytes the key portion of every row's item stores: the attribute name and its value.
fn key_bytes() -> usize {
    PARTITION.len() + KEY_FILL
}

/// Read the item under `tag` back, as a `ConsistentRead` reports it.
async fn read_item(router: &MultiServiceRouter, tag: &str) -> Value {
    ok(
        router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key_of(tag), "ConsistentRead": true}),
    )
    .await
}

/// The length of one string attribute of the item under `tag`, or `None` where the item holds
/// no such attribute.
async fn attribute_length(router: &MultiServiceRouter, tag: &str, attribute: &str) -> Option<usize> {
    read_item(router, tag).await["Item"][attribute]["S"]
        .as_str()
        .map(str::len)
}
/// One way of growing a stored item: the attributes the item is SEEDED with, each a name and
/// the bytes its value holds. Every row states ONE clause, so the update path charges the
/// request nothing of its own and the ceiling it is held to is the quota itself.
struct Growth {
    case: &'static str,
    /// The tag each row's keys are built from, so every row addresses its own items.
    tag: &'static str,
    /// The attributes the stored item carries, which the change below names none of.
    seeded: &'static [(&'static str, usize)],
}

/// The ways a change can be carried past the quota by the bytes the stored item already held.
/// They differ in how much the item holds beside the change.
const GROWTHS: &[Growth] = &[
    Growth {
        case: "a stored attribute of five bytes",
        tag: "g1",
        seeded: &[(HELD, 5)],
    },
    Growth {
        case: "a stored attribute of 100 KB",
        tag: "g2",
        seeded: &[(HELD, 100 * 1024)],
    },
    Growth {
        case: "two stored attributes the change names neither of",
        tag: "g3",
        seeded: &[(HELD, 1024), (KEPT, 2048)],
    },
];

impl Growth {
    /// The bytes the seeded item stores: its key portion and each attribute it carries.
    fn seeded_bytes(&self) -> usize {
        key_bytes()
            + self
                .seeded
                .iter()
                .map(|(attribute, bytes)| attribute.len() + bytes)
                .sum::<usize>()
    }

    /// The bytes the change itself writes beside the operand that grows the item: the grown
    /// attribute's name.
    fn written_overhead(&self) -> usize {
        GROWN.len()
    }

    /// The operand that leaves the item holding `total` bytes.
    fn operand_for(&self, total: usize) -> usize {
        total - self.seeded_bytes() - self.written_overhead()
    }

    /// `PutItem` the seeded item under this row's key tagged `leg`.
    async fn seed(&self, router: &MultiServiceRouter, leg: &str) -> String {
        let tag = format!("{}-{leg}", self.tag);
        let mut item = key_of(&tag);
        for (attribute, bytes) in self.seeded {
            item[*attribute] = json!({"S": "h".repeat(*bytes)});
        }
        ok(
            router,
            "PutItem",
            json!({"TableName": TABLE, "Item": item}),
        )
        .await;
        tag
    }

    /// The `TransactWriteItems` body whose `Update` grows the item under `tag` to `total`
    /// bytes.
    fn transaction(&self, tag: &str, total: usize) -> Value {
        json!({"TransactItems": [{"Update": {
            "TableName": TABLE,
            "Key": key_of(tag),
            "UpdateExpression": format!("SET {GROWN} = :v"),
            "ExpressionAttributeValues": {
                ":v": {"S": "x".repeat(self.operand_for(total))},
            },
        }}]})
    }
}

/// RULE: a change whose own statement fits in an item and that leaves the item past the quota
/// over the bytes the stored item already held CANCELS the transaction — the reason list names
/// that action with the sentence the update path words the quota with — and the stored item is
/// left exactly as it was.
#[tokio::test]
async fn a_growth_the_stored_bytes_carry_past_the_quota_cancels_the_transaction() {
    let router = seeded().await;

    for growth in GROWTHS {
        let tag = growth.seed(&router, "past").await;
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            growth.transaction(&tag, MAX_ITEM_BYTES + 1),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "TransactionCanceledException", CANCELLED_MESSAGE),
            "case {:?}: expected the transaction cancelled over that action, got {status} with \
             {body}",
            growth.case
        );
        assert_eq!(
            (
                body["CancellationReasons"][0]["Code"].as_str(),
                body["CancellationReasons"][0]["Message"].as_str(),
            ),
            (Some(VALIDATION_ERROR_CODE), Some(UPDATE_TOO_BIG)),
            "case {:?}: expected that action's reason to word the quota, got {body}",
            growth.case
        );

        // Nothing of the cancelled transaction landed: the item is what it was seeded with,
        // and the attribute the change would have written is not there.
        for (attribute, bytes) in growth.seeded {
            let held = attribute_length(&router, &tag, attribute).await;
            assert_eq!(
                held,
                Some(*bytes),
                "case {:?}: a cancelled transaction leaves {attribute} where it was, expected \
                 {bytes} byte(s), got {held:?}",
                growth.case
            );
        }
        let grown = attribute_length(&router, &tag, GROWN).await;
        assert_eq!(
            grown, None,
            "case {:?}: a cancelled transaction writes nothing, got {grown:?} byte(s) of \
             {GROWN}",
            growth.case
        );
    }
}

/// The same rows one byte UNDER that boundary: the change that leaves the item filling the
/// quota exactly is applied through the transaction and reads back whole, so the
/// cancellations above are the quota and not a blanket refusal.
#[tokio::test]
async fn the_change_that_fills_the_quota_exactly_commits_and_reads_back() {
    let router = seeded().await;

    for growth in GROWTHS {
        let tag = growth.seed(&router, "fills").await;
        ok(
            &router,
            "TransactWriteItems",
            growth.transaction(&tag, MAX_ITEM_BYTES),
        )
        .await;
        let expected = growth.operand_for(MAX_ITEM_BYTES);
        let grown = attribute_length(&router, &tag, GROWN).await;
        assert_eq!(
            grown,
            Some(expected),
            "case {:?}: expected the committed change to read back whole, {expected} byte(s) \
             of {GROWN}, got {grown:?}",
            growth.case
        );
    }
}

/// RULE, the request's own side: where the update path charges the REQUEST something beside
/// the bytes the item it leaves stores — the part of the item its clauses leave alone below
/// the floor that part is charged at, a further clause's own frame — the request was held to a
/// ceiling UNDER the quota that no stored item can lift, so a statement crossing it is refused
/// as the request it is: HTTP 400, the validation class, the update path's sentence and no
/// reason list at all, exactly as the single-item `UpdateItem` of the same statement is
/// refused. The boundary is driven from both sides, and the quota's own count is nowhere
/// crossed in these rows: every item they leave FITS it.
#[tokio::test]
async fn a_request_charged_a_ceiling_of_its_own_refuses_the_call() {
    let router = seeded().await;
    create_active_table(&router, SHORT_TABLE, &[(SHORT_PARTITION, "HASH", "S")]).await;
    let short_key = json!({SHORT_PARTITION: {"S": SHORT_KEY_VALUE}});
    ok(
        &router,
        "PutItem",
        json!({"TableName": SHORT_TABLE, "Item": short_key.clone()}),
    )
    .await;

    // The key portion is all these changes leave alone, and it holds less than the floor it is
    // charged at, so the ceiling this request is held to is the quota less what that portion is
    // short by.
    let short_key_bytes = SHORT_PARTITION.len() + SHORT_KEY_VALUE.len();
    let ceiling = MAX_ITEM_BYTES - (UNTOUCHED_PORTION_FLOOR - short_key_bytes);
    let largest = ceiling - short_key_bytes - GROWN.len();
    let short_update = |operand: usize| {
        json!({"TableName": SHORT_TABLE, "Key": short_key.clone(),
               "UpdateExpression": format!("SET {GROWN} = :v"),
               "ExpressionAttributeValues": {":v": {"S": "x".repeat(operand)}}})
    };

    // At that ceiling the change is served and reads back whole, so what follows is the
    // ceiling and not a blanket refusal.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": short_update(largest)}]}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": SHORT_TABLE, "Key": short_key.clone(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"][GROWN]["S"].as_str().map(str::len),
        Some(largest),
        "case \"the largest change the request's own ceiling admits\": expected the whole \
         operand stored, got {read}"
    );

    // One byte past it the CALL is refused — the item it would leave still fits the quota's own
    // count, so nothing became too large — and the same statement is refused the same way
    // through the single-item surface.
    for (case, request) in [
        (
            "a key portion below its floor, one byte past the ceiling that floor leaves",
            short_update(largest + 1),
        ),
        (
            // A second clause carries its own frame whatever the item holds, so this request is
            // charged a ceiling of its own even though the part of the item it leaves alone is
            // far past the floor.
            "a second clause stated beside the one that fills the quota",
            second_clause_update(MAX_ITEM_BYTES),
        ),
    ] {
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": request.clone()}]}),
        )
        .await;
        assert_eq!(
            (status, error_type(&body).as_str(), message(&body).as_str()),
            (400, VALIDATION_SHAPE_ID, UPDATE_TOO_BIG),
            "case {case:?}: expected 400 {VALIDATION_SHAPE_ID} {UPDATE_TOO_BIG:?}, got {status} \
             with {body}"
        );
        assert!(
            body.get("CancellationReasons").is_none(),
            "case {case:?}: a refused call grades no action, so it reports no reason list, got \
             {body}"
        );

        let (status, body) = call(&router, "UpdateItem", request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", UPDATE_TOO_BIG),
            "case {case:?}: expected UpdateItem to refuse the same statement, got {status} with \
             {body}"
        );
    }

    // Nothing either refusal carried was applied: the short-key item still holds what the
    // served change left it.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": SHORT_TABLE, "Key": short_key, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"][GROWN]["S"].as_str().map(str::len),
        Some(largest),
        "case \"after the refusals\": a refused change leaves the stored item where it was, got \
         {read}"
    );
}

/// The two-clause change that leaves the item under [`SECOND_CLAUSE_TAG`] holding `total`
/// bytes: the growing clause, and a second one writing a single byte into [`KEPT`].
///
/// The item it addresses holds nothing, so what it leaves alone is its key portion, which
/// fills the floor — the only thing the update path charges this request for is the second
/// clause's own frame.
fn second_clause_update(total: usize) -> Value {
    let operand = total - key_bytes() - GROWN.len() - KEPT.len() - 1;
    json!({"TableName": TABLE, "Key": key_of(SECOND_CLAUSE_TAG),
           "UpdateExpression": format!("SET {GROWN} = :v, {KEPT} = :w"),
           "ExpressionAttributeValues": {
               ":v": {"S": "x".repeat(operand)},
               ":w": {"S": "y"},
           }})
}

/// RULE, the carried side: an item a `Put` states WHOLE is weighed as the request's own, so
/// the item filling the quota exactly is stored and reads back whole through the transaction
/// and the one byte past it is refused as the request it is, worded for an item that was
/// carried.
#[tokio::test]
async fn an_item_a_put_carries_is_weighed_as_the_request_it_is() {
    let router = seeded().await;
    let carried = |tag: &str, total: usize| {
        let overhead = key_bytes() + HELD.len();
        json!({"TransactItems": [{"Put": {"TableName": TABLE, "Item": {
            PARTITION: {"S": key_value(tag)},
            HELD: {"S": "h".repeat(total - overhead)},
        }}}]})
    };

    ok(&router, "TransactWriteItems", carried("p1", MAX_ITEM_BYTES)).await;
    let expected = MAX_ITEM_BYTES - key_bytes() - HELD.len();
    let held = attribute_length(&router, "p1", HELD).await;
    assert_eq!(
        held,
        Some(expected),
        "case \"an item filling the quota exactly\": expected it stored whole, {expected} \
         byte(s), got {held:?}"
    );

    let (status, body) = call(
        &router,
        "TransactWriteItems",
        carried("p2", MAX_ITEM_BYTES + 1),
    )
    .await;
    assert_eq!(
        (status, error_type(&body).as_str(), message(&body).as_str()),
        (400, VALIDATION_SHAPE_ID, ITEM_TOO_BIG),
        "case \"an item one byte past the quota\": expected 400 {VALIDATION_SHAPE_ID} \
         {ITEM_TOO_BIG:?}, got {status} with {body}"
    );
    assert!(
        body.get("CancellationReasons").is_none(),
        "case \"an item one byte past the quota\": a refused call grades no action, got {body}"
    );
    let read = read_item(&router, "p2").await;
    assert!(
        read.get("Item").is_none(),
        "case \"an item one byte past the quota\": the refused put stored nothing, got {read}"
    );
}
