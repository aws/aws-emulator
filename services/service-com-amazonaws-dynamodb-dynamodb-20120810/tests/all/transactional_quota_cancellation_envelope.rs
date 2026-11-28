// SPDX-License-Identifier: CC0-1.0

//! The ENVELOPE a `TransactWriteItems` answers with when a change the transaction itself makes
//! leaves an item past the documented item-size quota, driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<Operation>`).
//!
//! RULE: where an action's change is read against the item its key HOLDS and leaves that item
//! past the quota over bytes the stored item already held — the request itself asking for
//! nothing an item could not hold — the crossing is "an item size becomes too large (larger
//! than 400 KB), or a local secondary index (LSI) becomes too large, or a similar validation
//! error occurs because of changes made by the transaction", one of the circumstances
//! `TransactionCanceledException` documents. So the transaction is CANCELLED as a unit rather
//! than the request refused, and the whole cancellation envelope is what the caller reads:
//! HTTP 400, a `__type` naming that error shape, a top-level `Message` of the fixed sentence
//! followed by the bracketed per-slot codes in request order, and `CancellationReasons` with
//! one reason per `TransactItems` slot in that same order — "an ordered list of errors for each
//! item in the request" — the offending slot reporting `Code` `ValidationError` with the
//! sentence the update path words that quota with, and every slot nothing was wrong with
//! reporting "an error with a Null code and Null message" (`Code` `None`, no `Message` member).
//! Nothing of a cancelled transaction is applied.
//!
//! The rows span what the rule generalises over: the CLAUSE that carries the item across (an
//! assignment writing an attribute the item does not hold, an assignment replacing one it does,
//! an `ADD` of a set member), the table's KEY SCHEMA (a partition key alone, and a composite
//! key whose two attributes are both left alone by the change), and WHICH bytes the quota counts
//! (the item's own, and the entry an `ALL`-projected local secondary index holds beside it — the
//! LSI circumstance the same sentence covers). The slot rows span the number of slots and where
//! the offending ones sit: the offending slot alone, between two blameless ones, beside a
//! condition that holds, and two offending slots in one transaction.
//!
//! The boundaries are driven beside them, so the cancellations are the quota and not a blanket
//! refusal: the same statement shape with a payload that leaves the item INSIDE the quota
//! commits through the transaction and reads back, and the very statement the indexed row is
//! cancelled for commits against the same table without the index — which is what shows the
//! index entry is the part of the charge that crossed.
//!
//! Source of the quota: "Service, Account, and Table Quotas" in the Amazon DynamoDB Developer
//! Guide ("The maximum item size in DynamoDB is 400 KB", and the per-local-index limit on "the
//! data in the table item, plus the index key values and projected attributes in the index
//! entry"), counted the way the guide counts an item: "the sum of the lengths of its attribute
//! names and values". Source of the cancel circumstance and of the ordered reason list:
//! `TransactWriteItems`' own `TransactionCanceledException` and `CancellationReason`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_type, message, ok};

/// The documented item quota, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// AWS's fixed sentence for a change that would leave the item past the quota.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The whole `__type` a cancelled transaction carries, which is what a client branching on the
/// error class matches.
const CANCELLED_TYPE: &str = "com.amazonaws.dynamodb.v20120810#TransactionCanceledException";

/// The `CancellationReason$Code` a slot that broke a rule of the service reports.
const VALIDATION_ERROR_CODE: &str = "ValidationError";

/// The `CancellationReason$Code` a slot nothing was wrong with reports.
const NO_REASON_CODE: &str = "None";

/// The table with a partition key alone.
const SIMPLE: &str = "TxnCancelEnvelopeSimple";

/// The table with a composite key schema, so what a change leaves alone spans two key
/// attributes.
const COMPOSITE: &str = "TxnCancelEnvelopeComposite";

/// The composite-keyed table carrying an `ALL`-projected local secondary index, whose entry the
/// quota counts beside the item.
const INDEXED: &str = "TxnCancelEnvelopeIndexed";

/// The same table WITHOUT that index: the control that says which bytes crossed the quota.
const UNINDEXED: &str = "TxnCancelEnvelopeUnindexed";

/// The local secondary index the indexed table declares.
const INDEX: &str = "by-local-sort";

/// How many bytes every key VALUE below holds. Twenty, so the part of an item a change leaves
/// alone is past the floor the update path charges a short one at
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`) and the ceiling every row drives is the quota
/// itself.
const KEY_VALUE_BYTES: usize = 20;

/// The bytes a stored item carries while a change grows it past the quota, and the bytes that
/// change writes: three quarters and three eighths of the quota, so the item the change leaves
/// is an eighth of the quota PAST it while neither the stored item nor the payload the request
/// carries comes near it on its own.
const STORED_BYTES: usize = MAX_ITEM_BYTES * 3 / 4;
const WRITTEN_BYTES: usize = MAX_ITEM_BYTES * 3 / 8;

/// The same two for the row driven against the indexed table, where the quota counts the index
/// entry beside the item: two fifths stored and three tenths written leave an item of seven
/// tenths of the quota, which fits it on its own and crosses it only together with the entry an
/// `ALL` projection holds for it.
const INDEXED_STORED_BYTES: usize = MAX_ITEM_BYTES * 2 / 5;
const INDEXED_WRITTEN_BYTES: usize = MAX_ITEM_BYTES * 3 / 10;

/// A payload that leaves a stored item of [`STORED_BYTES`] INSIDE the quota: an eighth of it,
/// which is the served side of the boundary the rows above cross.
const INSIDE_THE_QUOTA_BYTES: usize = MAX_ITEM_BYTES / 8;

/// `text` as a [`KEY_VALUE_BYTES`]-byte key value, so every key attribute holds the same number
/// of bytes whatever a row calls it. A longer `text` would be two keys collapsed into one, so
/// it is refused rather than cut.
fn padded(text: &str) -> String {
    assert!(
        text.len() <= KEY_VALUE_BYTES,
        "case {text:?}: a key value this suite pads is at most {KEY_VALUE_BYTES} bytes long"
    );
    let mut value = text.to_string();
    while value.len() < KEY_VALUE_BYTES {
        value.push('0');
    }
    value
}

/// A string attribute value of `bytes` single-byte characters.
fn filler(bytes: usize) -> Value {
    json!({"S": "x".repeat(bytes)})
}

/// The key map addressing `tag` in a table keyed by its partition key alone.
fn simple_key(tag: &str) -> Value {
    json!({"pk": {"S": padded(tag)}})
}

/// The key map addressing `tag` in a composite-keyed table.
fn composite_key(tag: &str) -> Value {
    json!({"pk": {"S": padded(tag)}, "sk": {"S": padded("sort")}})
}

/// The item a `PutItem` seeds: the key map, plus each attribute named beside it.
fn item_of(key: &Value, attributes: &[(&str, Value)]) -> Value {
    let mut item = key.clone();
    for (attribute, value) in attributes {
        item[*attribute] = value.clone();
    }
    item
}

/// The top-level sentence a cancelled transaction carries: fixed text, followed by the per-slot
/// codes in request order, which is the one part of it the request decides.
fn cancelled_message(codes: &[&str]) -> String {
    format!(
        "Transaction cancelled, please refer cancellation reasons for specific reasons [{}]",
        codes.join(", ")
    )
}

/// The `CreateTable` request for one of this suite's tables.
fn create_request(name: &str, composite: bool, indexed: bool) -> Value {
    let mut definitions = vec![json!({"AttributeName": "pk", "AttributeType": "S"})];
    let mut schema = vec![json!({"AttributeName": "pk", "KeyType": "HASH"})];
    if composite {
        definitions.push(json!({"AttributeName": "sk", "AttributeType": "S"}));
        schema.push(json!({"AttributeName": "sk", "KeyType": "RANGE"}));
    }
    let mut request = json!({
        "TableName": name,
        "KeySchema": schema,
        "BillingMode": "PAY_PER_REQUEST",
    });
    if indexed {
        definitions.push(json!({"AttributeName": "ls", "AttributeType": "S"}));
        request["LocalSecondaryIndexes"] = json!([{
            "IndexName": INDEX,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "ls", "KeyType": "RANGE"},
            ],
            "Projection": {"ProjectionType": "ALL"},
        }]);
    }
    request["AttributeDefinitions"] = Value::Array(definitions);
    request
}

/// An emulator holding every table the rows are driven against, each waited out through the
/// harness's bounded poll for the documented `CREATING` -> `ACTIVE` transition. Every create is
/// submitted before the first poll, so the four of them settle over one span rather than four.
async fn tables() -> MultiServiceRouter {
    let router = emulator();
    let declared = [
        (SIMPLE, false, false),
        (COMPOSITE, true, false),
        (INDEXED, true, true),
        (UNINDEXED, true, false),
    ];
    for (name, composite, indexed) in declared {
        ok(&router, "CreateTable", create_request(name, composite, indexed)).await;
    }
    for (name, _, _) in declared {
        describe_when_active(&router, name, name).await;
    }
    router
}

/// `PutItem` `item` into `table`.
async fn seed(router: &MultiServiceRouter, table: &str, item: Value) {
    ok(router, "PutItem", json!({"TableName": table, "Item": item})).await;
}

/// What the stored item under `key` holds at `attribute`, as a `ConsistentRead` reports it, and
/// `Value::Null` where the item — or the attribute — is not there.
async fn stored_attribute(
    router: &MultiServiceRouter,
    table: &str,
    key: &Value,
    attribute: &str,
) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": key.clone(), "ConsistentRead": true}),
    )
    .await;
    read["Item"][attribute].clone()
}

/// What a cancelled transaction answers, read as a client reads it: the status, the whole
/// `__type` it branches on, the top-level sentence over the per-slot codes in request order,
/// and one reason per slot — `messages` is the sentence each slot reports, `None` for a slot
/// nothing was wrong with, which carries no `Message` member at all.
fn assert_cancelled(
    case: &str,
    status: u16,
    body: &Value,
    codes: &[&str],
    messages: &[Option<&str>],
) {
    let sentence = cancelled_message(codes);
    assert_eq!(
        codes.len(),
        messages.len(),
        "case {case:?}: a row states one expected code per expected message"
    );
    assert_eq!(
        (status, error_type(body).as_str(), message(body).as_str()),
        (400, CANCELLED_TYPE, sentence.as_str()),
        "case {case:?}: expected 400 {CANCELLED_TYPE} {sentence:?}, got {status} with {body}"
    );
    let reported: Vec<(Option<&str>, Option<&str>)> = body["CancellationReasons"]
        .as_array()
        .map(|reasons| {
            reasons
                .iter()
                .map(|reason| {
                    (
                        reason["Code"].as_str(),
                        reason.get("Message").and_then(Value::as_str),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let expected: Vec<(Option<&str>, Option<&str>)> = codes
        .iter()
        .zip(messages)
        .map(|(code, message)| (Some(*code), *message))
        .collect();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the ordered reason list {expected:?}, got {reported:?} in \
         {body}"
    );
}

/// The blameless slots one row of the slot-grading table carries, so each row asserts only
/// what it actually submitted: a `Put` whose key must still hold nothing, and a `Delete` whose
/// item must still be there.
struct Blameless {
    put: bool,
    deleted: bool,
}

/// One way a transaction's own change carries an item past the quota.
struct Crossing {
    case: &'static str,
    table: &'static str,
    /// The primary key the change addresses.
    key: Value,
    /// The item stored under it before the change, its key included.
    item: Value,
    /// The `UpdateExpression` and the values it binds.
    statement: Value,
    /// The attribute that statement writes, and what the stored item holds there — which a
    /// cancelled transaction must leave exactly as it was (`Value::Null` where the item holds
    /// no such attribute at all).
    written: &'static str,
    held: Value,
}

impl Crossing {
    /// The one-action transaction this crossing states.
    fn transaction(&self) -> Value {
        json!({"TransactItems": [{"Update": self.action()}]})
    }

    /// The `Update` action itself, which the single-item surface takes as its own request too.
    fn action(&self) -> Value {
        let mut action = json!({"TableName": self.table, "Key": self.key.clone()});
        for (member, value) in self
            .statement
            .as_object()
            .expect("a crossing states its statement as members")
        {
            action[member.as_str()] = value.clone();
        }
        action
    }
}

/// The ways a change read against the stored item leaves it past the quota. They differ in the
/// clause that carries it across, in the table's key schema, and in which bytes the quota
/// counts — the item's own, and the local secondary index entry counted beside it.
fn crossings() -> Vec<Crossing> {
    let held = ("held", filler(STORED_BYTES));
    vec![
        Crossing {
            case: "an assignment writing an attribute the item does not hold",
            table: SIMPLE,
            key: simple_key("fresh"),
            item: item_of(&simple_key("fresh"), std::slice::from_ref(&held)),
            statement: json!({"UpdateExpression": "SET grow = :v",
                              "ExpressionAttributeValues": {":v": filler(WRITTEN_BYTES)}}),
            written: "grow",
            held: Value::Null,
        },
        Crossing {
            case: "an assignment replacing an attribute the item does hold",
            table: SIMPLE,
            key: simple_key("replaced"),
            item: item_of(
                &simple_key("replaced"),
                &[held.clone(), ("small", filler(1))],
            ),
            statement: json!({"UpdateExpression": "SET small = :v",
                              "ExpressionAttributeValues": {":v": filler(WRITTEN_BYTES)}}),
            written: "small",
            held: filler(1),
        },
        Crossing {
            case: "an ADD of one member into a set the item already holds",
            table: SIMPLE,
            key: simple_key("added"),
            item: item_of(
                &simple_key("added"),
                &[held.clone(), ("tags", json!({"SS": ["one"]}))],
            ),
            statement: json!({"UpdateExpression": "ADD tags :more",
                              "ExpressionAttributeValues": {
                                  ":more": {"SS": ["x".repeat(WRITTEN_BYTES)]}}}),
            written: "tags",
            held: json!({"SS": ["one"]}),
        },
        Crossing {
            case: "a composite key schema, both of whose attributes the change leaves alone",
            table: COMPOSITE,
            key: composite_key("composite"),
            item: item_of(&composite_key("composite"), std::slice::from_ref(&held)),
            statement: json!({"UpdateExpression": "SET grow = :v",
                              "ExpressionAttributeValues": {":v": filler(WRITTEN_BYTES)}}),
            written: "grow",
            held: Value::Null,
        },
        Crossing {
            case: "an item the quota counts an ALL-projected local index entry beside",
            table: INDEXED,
            key: composite_key("indexed"),
            item: item_of(
                &composite_key("indexed"),
                &[
                    ("ls", json!({"S": padded("local")})),
                    ("held", filler(INDEXED_STORED_BYTES)),
                ],
            ),
            statement: json!({"UpdateExpression": "SET grow = :v",
                              "ExpressionAttributeValues": {
                                  ":v": filler(INDEXED_WRITTEN_BYTES)}}),
            written: "grow",
            held: Value::Null,
        },
    ]
}

/// RULE: a change read against the stored item that leaves it past the quota over bytes that
/// item already held CANCELS the transaction — the cancellation class, the top-level sentence
/// over that slot's code, and an ordered reason list naming the slot with the sentence the
/// update path words the quota with — and the stored item is left exactly as it was.
#[tokio::test]
async fn a_change_the_stored_bytes_carry_past_the_quota_cancels_the_transaction() {
    let router = tables().await;

    for crossing in crossings() {
        seed(&router, crossing.table, crossing.item.clone()).await;
        let (status, body) = call(&router, "TransactWriteItems", crossing.transaction()).await;
        assert_cancelled(
            crossing.case,
            status,
            &body,
            &[VALIDATION_ERROR_CODE],
            &[Some(UPDATE_TOO_BIG)],
        );

        // Nothing of the cancelled transaction landed: the attribute its clause addressed holds
        // what the stored item held there, and no more.
        let stored = stored_attribute(&router, crossing.table, &crossing.key, crossing.written)
            .await;
        assert_eq!(
            stored, crossing.held,
            "case {:?}: a cancelled transaction leaves {} where it was, expected {:?}, got \
             {stored:?}",
            crossing.case, crossing.written, crossing.held
        );
    }
}

/// RULE: the reason list is "an ordered list of errors for each item in the request [...]
/// ordered according to the ordering of the TransactItems request parameter", so every slot is
/// graded and sits where the request put it — the offending ones with the quota's sentence and
/// every other one with the null code and no message — and the bracketed list in the top-level
/// sentence is that same list of codes.
#[tokio::test]
async fn every_slot_of_a_cancelled_transaction_is_graded_in_request_order() {
    let router = tables().await;

    // Two keys whose items each sit three quarters of the quota from empty, so the change below
    // crosses it over the bytes they already hold, and one ordinary item the blameless slots
    // address.
    for tag in ["graded-one", "graded-two"] {
        seed(
            &router,
            SIMPLE,
            item_of(&simple_key(tag), &[("held", filler(STORED_BYTES))]),
        )
        .await;
    }
    seed(&router, SIMPLE, simple_key("present")).await;

    let oversize = |tag: &str| {
        json!({"Update": {"TableName": SIMPLE, "Key": simple_key(tag),
               "UpdateExpression": "SET grow = :v",
               "ExpressionAttributeValues": {":v": filler(WRITTEN_BYTES)}}})
    };
    let put = |tag: &str| json!({"Put": {"TableName": SIMPLE, "Item": simple_key(tag)}});
    let delete = |tag: &str| json!({"Delete": {"TableName": SIMPLE, "Key": simple_key(tag)}});
    let holds = |tag: &str| {
        json!({"ConditionCheck": {"TableName": SIMPLE, "Key": simple_key(tag),
               "ConditionExpression": "attribute_exists(pk)"}})
    };

    for (case, actions, codes, messages, blameless) in [
        (
            "the offending slot alone",
            json!([oversize("graded-one")]),
            vec![VALIDATION_ERROR_CODE],
            vec![Some(UPDATE_TOO_BIG)],
            Blameless {
                put: false,
                deleted: false,
            },
        ),
        (
            "the offending slot between two blameless ones",
            json!([
                put("blameless"),
                oversize("graded-one"),
                delete("present")
            ]),
            vec![NO_REASON_CODE, VALIDATION_ERROR_CODE, NO_REASON_CODE],
            vec![None, Some(UPDATE_TOO_BIG), None],
            Blameless {
                put: true,
                deleted: true,
            },
        ),
        (
            "a condition that holds before the offending slot",
            json!([holds("present"), oversize("graded-one")]),
            vec![NO_REASON_CODE, VALIDATION_ERROR_CODE],
            vec![None, Some(UPDATE_TOO_BIG)],
            Blameless {
                put: false,
                deleted: false,
            },
        ),
        (
            "two offending slots in one transaction",
            json!([oversize("graded-one"), oversize("graded-two")]),
            vec![VALIDATION_ERROR_CODE, VALIDATION_ERROR_CODE],
            vec![Some(UPDATE_TOO_BIG), Some(UPDATE_TOO_BIG)],
            Blameless {
                put: false,
                deleted: false,
            },
        ),
    ] {
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": actions}),
        )
        .await;
        assert_cancelled(case, status, &body, &codes, &messages);

        // Nothing a cancelled transaction carried was applied, the blameless slots included —
        // asserted for the rows that CARRY one, so each assertion is a row's own: the key a
        // `Put` would have written holds nothing, and the item a `Delete` would have taken
        // away is still there.
        if blameless.put {
            let written = stored_attribute(&router, SIMPLE, &simple_key("blameless"), "pk").await;
            assert_eq!(
                written,
                Value::Null,
                "case {case:?}: a cancelled transaction writes nothing, got {written:?} under \
                 the key its Put addressed"
            );
        }
        if blameless.deleted {
            let kept = stored_attribute(&router, SIMPLE, &simple_key("present"), "pk").await;
            assert_eq!(
                kept,
                json!({"S": padded("present")}),
                "case {case:?}: a cancelled transaction deletes nothing, got {kept:?} under the \
                 key its Delete addressed"
            );
        }
    }
}

/// The served side of the boundary, so the cancellations above are the quota and not a blanket
/// refusal of a transactional change: the same statement shape with a payload that leaves the
/// item INSIDE the quota commits through the transaction and reads back whole.
#[tokio::test]
async fn a_change_inside_the_quota_commits_through_the_transaction() {
    let router = tables().await;
    seed(
        &router,
        SIMPLE,
        item_of(&simple_key("inside"), &[("held", filler(STORED_BYTES))]),
    )
    .await;

    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": {
            "TableName": SIMPLE, "Key": simple_key("inside"),
            "UpdateExpression": "SET grow = :v",
            "ExpressionAttributeValues": {":v": filler(INSIDE_THE_QUOTA_BYTES)}}}]}),
    )
    .await;
    let stored = stored_attribute(&router, SIMPLE, &simple_key("inside"), "grow").await;
    assert_eq!(
        stored["S"].as_str().map(str::len),
        Some(INSIDE_THE_QUOTA_BYTES),
        "case \"a change inside the quota\": expected the committed change to read back whole, \
         got {stored:?}"
    );
}

/// RULE, the LSI half of it: what crossed the quota in the indexed row is the entry the index
/// holds BESIDE the item, so the very statement that row is cancelled for commits against the
/// same table declared without that index — the item it leaves fits the quota on its own.
#[tokio::test]
async fn the_index_entry_is_what_the_quota_counted_beside_the_item() {
    let router = tables().await;
    let indexed = crossings()
        .into_iter()
        .find(|crossing| crossing.table == INDEXED)
        .expect("one row is driven against the indexed table");

    // The same item and the same statement, against the table that declares no local index.
    let unindexed = Crossing {
        case: "the indexed row's statement against a table with no local index",
        table: UNINDEXED,
        ..indexed
    };
    seed(&router, UNINDEXED, unindexed.item.clone()).await;
    ok(&router, "TransactWriteItems", unindexed.transaction()).await;
    let stored = stored_attribute(&router, UNINDEXED, &unindexed.key, unindexed.written).await;
    assert_eq!(
        stored["S"].as_str().map(str::len),
        Some(INDEXED_WRITTEN_BYTES),
        "case {:?}: expected the committed change to read back whole, got {stored:?}",
        unindexed.case
    );
}
