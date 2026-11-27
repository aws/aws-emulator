// SPDX-License-Identifier: CC0-1.0

//! Where an UPDATE's item-size boundary sits when the item carries attributes the change does
//! NOT address, driven end to end.
//!
//! THE RULE: a change stated as an `UpdateExpression` is weighed against the documented 400 KB
//! item quota over what the item it leaves behind is CHARGED, and what is charged is the bytes
//! of the attributes the change ADDRESSES plus the part of the item it leaves alone charged
//! together at a minimum of its own (`state::UNTOUCHED_PORTION_FLOOR_BYTES`). That part is
//! every attribute no clause names — the partition key, the sort key and any attribute the
//! change never touches, none of them excluded and none of them counted twice — so the largest
//! value one assignment may write is the quota less the attribute name it writes and less what
//! that untouched part is charged. The boundary value is applied whole and reads back at that
//! length; one byte more is refused with the sentence the update path words that quota with,
//! and the stored item keeps what it held.
//!
//! So an item whose untouched part holds fewer bytes than the floor buys no headroom from being
//! short — a two-byte key portion and a two-byte key portion beside a twenty-byte attribute top
//! out at the same value length — while one past the floor is charged the bytes it holds.
//!
//! The rows span what the rule generalises over: untouched parts below the floor, exactly on it,
//! one byte past it and well past it, made up of a short key, a long key value, a long key
//! NAME, a composite key, one untouched attribute beside the key and two of them.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB
//! Developer Guide ("The maximum item size in DynamoDB is 400 KB"); the floor under the
//! untouched part is measured behavior, stated once by the service
//! (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::UNTOUCHED_PORTION_FLOOR_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The documented item ceiling, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// AWS's fixed sentence for a change that would grow the stored item past the ceiling.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The attribute every row's change writes: one byte of name, so what a row varies is the
/// untouched part beside it.
const WRITTEN: &str = "v";

/// The floor the untouched part of an item is charged at, as a byte count.
fn floor() -> usize {
    usize::try_from(UNTOUCHED_PORTION_FLOOR_BYTES)
        .expect("the floor the untouched part of an item is charged at is a byte count")
}

/// One attribute's name and value, each as a number of single-byte characters.
#[derive(Debug, Clone, Copy)]
struct Attribute {
    name_bytes: usize,
    value_bytes: usize,
}

impl Attribute {
    /// The bytes this attribute stores: its name and its value.
    fn bytes(self) -> usize {
        self.name_bytes + self.value_bytes
    }

    /// The attribute's name, `lead` followed by filler so no two names of one item collide.
    fn name(self, lead: char) -> String {
        format!("{lead}{}", "n".repeat(self.name_bytes - 1))
    }

    /// The attribute's value, as a String of single-byte characters.
    fn value(self) -> String {
        "0".repeat(self.value_bytes)
    }
}

/// One row: the table's key shape and the attributes its item carries that no clause of the
/// change names.
struct Row {
    case: &'static str,
    table: &'static str,
    partition: Attribute,
    /// The sort key attribute, where the table declares one.
    sort: Option<Attribute>,
    /// The attributes the item carries that the change leaves alone.
    untouched: &'static [Attribute],
}

impl Row {
    /// The bytes the part of this row's item the change does not address holds: every key
    /// attribute's name and value, and every untouched attribute's name and value.
    fn untouched_bytes(&self) -> usize {
        self.partition.bytes()
            + self.sort.map_or(0, Attribute::bytes)
            + self
                .untouched
                .iter()
                .map(|attribute| attribute.bytes())
                .sum::<usize>()
    }

    /// What that part is CHARGED: its own bytes once they fill the floor, the floor while they
    /// do not.
    fn untouched_charge(&self) -> usize {
        self.untouched_bytes().max(floor())
    }

    /// The largest value `SET v = :v` may write into this row's item: the quota less the
    /// attribute name the clause writes and less what the untouched part is charged.
    fn largest_value(&self) -> usize {
        MAX_ITEM_BYTES - WRITTEN.len() - self.untouched_charge()
    }

    /// The partition key attribute's name, and the sort key's where there is one. The leads
    /// keep the names of one item apart.
    fn partition_name(&self) -> String {
        self.partition.name('p')
    }

    fn sort_name(&self) -> Option<String> {
        self.sort.map(|sort| sort.name('s'))
    }

    fn key_schema(&self) -> Vec<Value> {
        let mut schema = vec![json!({"AttributeName": self.partition_name(), "KeyType": "HASH"})];
        if let Some(name) = self.sort_name() {
            schema.push(json!({"AttributeName": name, "KeyType": "RANGE"}));
        }
        schema
    }

    fn attribute_definitions(&self) -> Vec<Value> {
        let mut definitions =
            vec![json!({"AttributeName": self.partition_name(), "AttributeType": "S"})];
        if let Some(name) = self.sort_name() {
            definitions.push(json!({"AttributeName": name, "AttributeType": "S"}));
        }
        definitions
    }

    /// The `Key` map this row's item is addressed by.
    fn key(&self) -> Value {
        let mut key = serde_json::Map::new();
        key.insert(
            self.partition_name(),
            json!({"S": self.partition.value()}),
        );
        if let (Some(name), Some(sort)) = (self.sort_name(), self.sort) {
            key.insert(name, json!({"S": sort.value()}));
        }
        Value::Object(key)
    }

    /// The untouched attributes as `(name, value)` pairs, each named after its position so no
    /// two collide.
    fn untouched_attributes(&self) -> Vec<(String, String)> {
        self.untouched
            .iter()
            .enumerate()
            .map(|(at, attribute)| {
                let lead = char::from(b'a' + u8::try_from(at).unwrap_or(0));
                (attribute.name(lead), attribute.value())
            })
            .collect()
    }

    /// The item the row is seeded with: its key and the attributes the change leaves alone.
    fn seed(&self) -> Value {
        let mut item = self.key();
        for (name, value) in self.untouched_attributes() {
            item[name] = json!({"S": value});
        }
        item
    }

    /// The `UpdateItem` that writes `bytes` single-byte characters into the written attribute.
    fn update(&self, bytes: usize) -> Value {
        json!({
            "TableName": self.table,
            "Key": self.key(),
            "UpdateExpression": "SET v = :v",
            "ExpressionAttributeValues": {":v": {"S": "x".repeat(bytes)}},
        })
    }
}

/// The smallest key an attribute can be: one byte of name beside one byte of value.
const ONE_BYTE_EACH: Attribute = Attribute {
    name_bytes: 1,
    value_bytes: 1,
};

/// The rows: untouched parts below the floor, exactly on it, one byte past it and well past
/// it, built out of keys and untouched attributes in turn.
const ROWS: &[Row] = &[
    Row {
        case: "a one-byte key name and a one-byte key value, nothing else",
        table: "UntouchedTinyKey",
        partition: ONE_BYTE_EACH,
        sort: None,
        untouched: &[],
    },
    Row {
        case: "an untouched attribute leaving the untouched part under the floor",
        table: "UntouchedUnderTheFloor",
        partition: ONE_BYTE_EACH,
        sort: None,
        untouched: &[Attribute {
            name_bytes: 1,
            value_bytes: 16,
        }],
    },
    Row {
        case: "an untouched attribute bringing the untouched part exactly onto the floor",
        table: "UntouchedOnTheFloor",
        partition: ONE_BYTE_EACH,
        sort: None,
        untouched: &[Attribute {
            name_bytes: 1,
            value_bytes: 19,
        }],
    },
    Row {
        case: "an untouched attribute one byte past the floor",
        table: "UntouchedPastTheFloor",
        partition: ONE_BYTE_EACH,
        sort: None,
        untouched: &[Attribute {
            name_bytes: 1,
            value_bytes: 20,
        }],
    },
    Row {
        case: "two untouched attributes well past the floor together",
        table: "UntouchedTwoAttributes",
        partition: ONE_BYTE_EACH,
        sort: None,
        untouched: &[
            Attribute {
                name_bytes: 1,
                value_bytes: 20,
            },
            Attribute {
                name_bytes: 3,
                value_bytes: 40,
            },
        ],
    },
    Row {
        case: "a hundred-byte key value and no other attribute",
        table: "UntouchedLongKeyValue",
        partition: Attribute {
            name_bytes: 1,
            value_bytes: 100,
        },
        sort: None,
        untouched: &[],
    },
    Row {
        case: "a hundred-byte key NAME and no other attribute",
        table: "UntouchedLongKeyName",
        partition: Attribute {
            name_bytes: 100,
            value_bytes: 1,
        },
        sort: None,
        untouched: &[],
    },
    Row {
        case: "a composite key whose sort key value holds a hundred bytes",
        table: "UntouchedCompositeKey",
        partition: Attribute {
            name_bytes: 2,
            value_bytes: 1,
        },
        sort: Some(Attribute {
            name_bytes: 2,
            value_bytes: 100,
        }),
        untouched: &[Attribute {
            name_bytes: 1,
            value_bytes: 20,
        }],
    },
];

/// Create every row's table, wait out the documented `CREATING` -> `ACTIVE` transition and
/// seed each row's item.
///
/// Every table is created first and waited for afterwards, so the transient span the clock owes
/// each of them runs down together rather than once per table.
async fn seeded_tables() -> MultiServiceRouter {
    let router = emulator();
    for row in ROWS {
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": row.table,
                "KeySchema": row.key_schema(),
                "AttributeDefinitions": row.attribute_definitions(),
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    }
    for row in ROWS {
        describe_when_active(&router, row.case, row.table).await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": row.table, "Item": row.seed()}),
        )
        .await;
    }
    router
}

/// RULE: the largest value an assignment may write is the quota less the attribute name it
/// writes and less what the part of the item it leaves alone is CHARGED — which is that part's
/// own bytes only once they fill the floor. The boundary value is stored whole; one byte more
/// is refused and the item keeps what it held.
#[tokio::test]
async fn an_update_tops_out_over_the_charged_untouched_part_of_its_item() {
    let router = seeded_tables().await;

    for row in ROWS {
        let case = row.case;
        let largest = row.largest_value();

        // At the boundary the change is applied, and the whole value is stored.
        let (status, body) = call(&router, "UpdateItem", row.update(largest)).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: a {largest}-byte value over a {}-byte untouched part charged as {} \
             must be applied, got {status} with {}",
            row.untouched_bytes(),
            row.untouched_charge(),
            message(&body)
        );
        let stored = stored_value_length(&router, row).await;
        assert_eq!(
            stored,
            Some(largest),
            "case {case:?}: expected the whole {largest}-byte value stored, got {stored:?} \
             byte(s)"
        );
        // Every untouched attribute is still there: the change addressed none of them, so the
        // accounting that charged them must not have disturbed them.
        let kept = stored_item(&router, row).await;
        for (name, value) in row.untouched_attributes() {
            assert_eq!(
                kept[&name],
                json!({"S": value}),
                "case {case:?}: the untouched attribute {name:?} must still be stored, got {}",
                kept[&name]
            );
        }

        // One byte past it the change is refused, and the stored value stays where it was.
        let (status, body) = call(&router, "UpdateItem", row.update(largest + 1)).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", UPDATE_TOO_BIG),
            "case {case:?}: a value one byte past the boundary must be refused, got {status} \
             with {body}"
        );
        let stored = stored_value_length(&router, row).await;
        assert_eq!(
            stored,
            Some(largest),
            "case {case:?}: a refused change leaves the stored value where it was, got \
             {stored:?} byte(s)"
        );
    }
}

/// RULE, read as a boundary that does not MOVE, over the boundaries the SERVICE answers with:
/// the largest value the reference item accepts is accepted by every other item whose untouched
/// part holds fewer bytes than the floor, however differently those bytes are made up, and the
/// byte past it is refused on each of them — while an item whose untouched part is PAST the
/// floor refuses that same length and accepts exactly the bytes past it fewer, so the floor is a
/// minimum and not a flat charge.
///
/// Every claim here is a call: the lengths are probed through `UpdateItem` on each item rather
/// than compared against this suite's own arithmetic.
#[tokio::test]
async fn the_boundary_the_service_answers_with_follows_the_charged_untouched_part() {
    let router = seeded_tables().await;

    let below: Vec<&Row> = ROWS
        .iter()
        .filter(|row| row.untouched_bytes() < floor())
        .collect();
    assert!(
        below.len() > 1,
        "the suite states this half over SEVERAL items under the floor, got {}",
        below.len()
    );
    // The REFERENCE boundary, measured on the first item under the floor: this length is
    // applied there and one byte more is not.
    let reference = below[0];
    let accepted = reference.largest_value();
    assert_eq!(
        applied(&router, reference, accepted).await,
        Some(accepted),
        "case {:?}: the reference boundary of {accepted} byte(s) must be applied",
        reference.case
    );
    assert_eq!(
        applied(&router, reference, accepted + 1).await,
        None,
        "case {:?}: one byte past the reference boundary must be refused",
        reference.case
    );

    // Every other item under the floor answers the same way at the same two lengths.
    for row in below.iter().skip(1) {
        assert_eq!(
            applied(&router, row, accepted).await,
            Some(accepted),
            "case {:?}: an untouched part of {} byte(s), under the {}-byte floor, must accept the \
             {accepted}-byte value the reference item accepts",
            row.case,
            row.untouched_bytes(),
            floor()
        );
        assert_eq!(
            applied(&router, row, accepted + 1).await,
            None,
            "case {:?}: it must refuse the byte past that boundary too",
            row.case
        );
    }

    // An item whose untouched part is past the floor has exactly those bytes less headroom:
    // the reference's own boundary is refused there, and that many bytes shorter is applied.
    for row in ROWS.iter().filter(|row| row.untouched_bytes() > floor()) {
        let past = row.untouched_bytes() - floor();
        assert_eq!(
            applied(&router, row, accepted).await,
            None,
            "case {:?}: an untouched part {past} byte(s) past the floor must refuse the \
             {accepted}-byte value an item under the floor accepts",
            row.case
        );
        assert_eq!(
            applied(&router, row, accepted - past).await,
            Some(accepted - past),
            "case {:?}: it must accept exactly {past} byte(s) fewer",
            row.case
        );
    }
}

/// Whether an `operand`-byte value is APPLIED to this row's item, reported as the length the
/// item is left holding, or `None` where the change was refused with the update path's own
/// ceiling sentence. A refusal worded any other way panics naming the row, so no other failure
/// reads as a boundary.
async fn applied(router: &MultiServiceRouter, row: &Row, operand: usize) -> Option<usize> {
    let (status, body) = call(router, "UpdateItem", row.update(operand)).await;
    if status == 200 {
        return stored_value_length(router, row).await;
    }
    assert_eq!(
        (status, error_code(&body).as_str(), message(&body).as_str()),
        (400, "ValidationException", UPDATE_TOO_BIG),
        "case {:?}: a {operand}-byte value must be either applied or refused with the update \
         path's ceiling sentence, got {status} with {body}",
        row.case
    );
    None
}

/// The item a `ConsistentRead` `GetItem` reports for a row's key.
async fn stored_item(router: &MultiServiceRouter, row: &Row) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": row.table, "Key": row.key(), "ConsistentRead": true}),
    )
    .await;
    read.get("Item").cloned().unwrap_or(Value::Null)
}

/// The length of the written attribute as that read reports it.
async fn stored_value_length(router: &MultiServiceRouter, row: &Row) -> Option<usize> {
    stored_item(router, row).await[WRITTEN]["S"]
        .as_str()
        .map(str::len)
}
