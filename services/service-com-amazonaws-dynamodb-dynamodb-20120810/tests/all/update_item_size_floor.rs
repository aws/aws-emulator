// SPDX-License-Identifier: CC0-1.0

//! The item-size ceiling an UPDATE is held to, whose KEY PORTION is charged at a floor where
//! the change is stated as an expression.
//!
//! The rule under test: a change is refused when what the item it would leave behind is
//! CHARGED crosses the documented 400 KB item ceiling, and what is charged is the bytes the
//! item stores — its attribute names and values, the key attributes among them — plus what the
//! SHAPE of the request stating the change costs: a change stated as an `UpdateExpression` is
//! charged its primary-key portion — the part these items leave untouched — at a minimum of
//! `state::UNTOUCHED_PORTION_FLOOR_BYTES` whatever
//! it actually holds, while a change stated without an expression — a PartiQL `UPDATE` — is
//! charged the item alone. So the largest change an item accepts is the ceiling less what the
//! rest of the item already holds and less the part of that floor its key does not fill; one
//! byte more is refused. A refused change leaves the stored item where it was, and the refusal
//! states the sentence the update path words that quota with.
//!
//! The rows span what the rule generalises over: key portions of four different sizes (a
//! one-byte name with a one-byte value, a longer short key, one landing exactly on the floor
//! and a long name beside a long value well past it) plus a composite-key table whose two key
//! attributes are charged together — each crossed with every surface that applies a stated
//! change to a stored item (`UpdateItem`, `TransactWriteItems`' `Update`, and a PartiQL
//! `UPDATE` through `ExecuteStatement`, `BatchExecuteStatement` and `ExecuteTransaction`), at
//! its own computed boundary and one byte past it.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB
//! Developer Guide ("The maximum item size in DynamoDB is 400 KB"); the key portion's floor is
//! measured behavior. The service states both once (`state::update_size_exceeded`) for every
//! update surface.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, message, ok};
use crate::table_items::create_active_table;

/// The documented item ceiling, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// The least an item's primary-key portion is charged against that ceiling when the change is
/// stated as an expression, as the service states it
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).
const KEY_PORTION_FLOOR: usize = 22;

/// AWS's fixed sentence for a change that would grow the stored item past the ceiling.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The attribute every row's change writes, one byte of name.
const WRITTEN: &str = "v";

/// The value a row seeds its item with before the update under test.
const SEEDED: &str = "start";

/// One surface that applies a stated CHANGE to a stored item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// The single-item update.
    UpdateItem,
    /// The `Update` action of a transaction, held to the same ceiling as the single-item
    /// update: it cancels the transaction with that action's reason, in that same sentence,
    /// where the item the change leaves is past the quota in the bytes it stores, and refuses
    /// the call where only what the request's own accounting charges crossed the ceiling.
    TransactWriteItems,
    /// A PartiQL `UPDATE` run on its own.
    ExecuteStatement,
    /// The same statement in a batch, which answers 200 and states the refusal in the slot
    /// that carried the statement.
    BatchExecuteStatement,
    /// The same statement in a PartiQL transaction.
    ExecuteTransaction,
}

const SURFACES: &[Surface] = &[
    Surface::UpdateItem,
    Surface::TransactWriteItems,
    Surface::ExecuteStatement,
    Surface::BatchExecuteStatement,
    Surface::ExecuteTransaction,
];

impl Surface {
    /// The operation name a row calls.
    fn operation(self) -> &'static str {
        match self {
            Surface::UpdateItem => "UpdateItem",
            Surface::TransactWriteItems => "TransactWriteItems",
            Surface::ExecuteStatement => "ExecuteStatement",
            Surface::BatchExecuteStatement => "BatchExecuteStatement",
            Surface::ExecuteTransaction => "ExecuteTransaction",
        }
    }

    /// The suffix a row's key value carries, so each surface writes its own item and no two
    /// rows read what the other wrote.
    fn suffix(self) -> &'static str {
        match self {
            Surface::UpdateItem => "u",
            Surface::TransactWriteItems => "t",
            Surface::ExecuteStatement => "s",
            Surface::BatchExecuteStatement => "b",
            Surface::ExecuteTransaction => "x",
        }
    }

    /// Whether a change stated on this surface is charged the key-portion floor: the two
    /// surfaces that state their change as an `UpdateExpression` are, and the PartiQL ones,
    /// which state no expression at all, are charged the item alone.
    fn charges_the_key_floor(self) -> bool {
        matches!(self, Surface::UpdateItem | Surface::TransactWriteItems)
    }

    /// The request that writes an operand of `operand` single-byte characters into `key`'s
    /// item on this surface.
    fn request(self, shape: &Shape, key: &Value, operand: usize) -> Value {
        let value = json!({"S": "x".repeat(operand)});
        match self {
            Surface::UpdateItem => json!({
                "TableName": shape.table, "Key": key,
                "UpdateExpression": "SET v = :v",
                "ExpressionAttributeValues": {":v": value},
            }),
            Surface::TransactWriteItems => json!({"TransactItems": [{"Update": {
                "TableName": shape.table, "Key": key,
                "UpdateExpression": "SET v = :v",
                "ExpressionAttributeValues": {":v": value},
            }}]}),
            Surface::ExecuteStatement => json!({
                "Statement": shape.partiql_update(key),
                "Parameters": [value],
            }),
            Surface::BatchExecuteStatement => json!({"Statements": [{
                "Statement": shape.partiql_update(key),
                "Parameters": [value],
            }]}),
            Surface::ExecuteTransaction => json!({"TransactStatements": [{
                "Statement": shape.partiql_update(key),
                "Parameters": [value],
            }]}),
        }
    }
}

/// One table shape: the key attributes and the key VALUES a row addresses its item by.
struct Shape {
    case: &'static str,
    table: &'static str,
    /// The partition key attribute's name, and the sort key's where the table has one.
    partition: &'static str,
    sort: Option<&'static str>,
    /// The key values the row's item carries, as single-byte strings.
    partition_value: &'static str,
    sort_value: Option<&'static str>,
}

impl Shape {
    /// The bytes this shape's key portion stores: each key attribute's name plus its value.
    fn key_bytes(&self) -> usize {
        self.partition.len()
            + self.partition_value.len()
            + self.sort.map_or(0, str::len)
            + self.sort_value.map_or(0, str::len)
    }

    fn key_schema(&self) -> Vec<(&'static str, &'static str, &'static str)> {
        let mut schema = vec![(self.partition, "HASH", "S")];
        if let Some(sort) = self.sort {
            schema.push((sort, "RANGE", "S"));
        }
        schema
    }

    /// The key of this shape's item for `surface`, whose own suffix keeps the surfaces apart.
    fn key_for(&self, surface: Surface) -> Value {
        let mut key = serde_json::Map::new();
        key.insert(
            self.partition.to_string(),
            json!({"S": format!("{}{}", self.partition_value, surface.suffix())}),
        );
        if let (Some(name), Some(value)) = (self.sort, self.sort_value) {
            key.insert(name.to_string(), json!({"S": value}));
        }
        Value::Object(key)
    }

    /// The PartiQL statement that writes the parameter into `key`'s item, addressing the item
    /// by every key attribute the table declares.
    fn partiql_update(&self, key: &Value) -> String {
        let mut predicate = format!(
            "{} = '{}'",
            self.partition,
            key[self.partition]["S"].as_str().unwrap_or_default()
        );
        if let Some(sort) = self.sort {
            predicate.push_str(&format!(
                " AND {sort} = '{}'",
                key[sort]["S"].as_str().unwrap_or_default()
            ));
        }
        format!(
            "UPDATE \"{}\" SET {WRITTEN} = ? WHERE {predicate}",
            self.table
        )
    }
}

/// Key portions below the floor, one landing exactly on it, one well past it, plus a composite
/// key charged across its two attributes. Each row's own value carries one more byte than the
/// shape states, which the surface's suffix adds.
const SHAPES: &[Shape] = &[
    Shape {
        case: "a one-byte key name and a one-byte key value",
        table: "CeilingTinyKey",
        partition: "k",
        sort: None,
        partition_value: "1",
        sort_value: None,
    },
    Shape {
        case: "a key portion of eight bytes",
        table: "CeilingShortKey",
        partition: "pk",
        sort: None,
        partition_value: "123456",
        sort_value: None,
    },
    Shape {
        case: "a key portion landing exactly on the floor",
        table: "CeilingKeyOnTheFloor",
        partition: "pk",
        sort: None,
        partition_value: "1234567890123456789",
        sort_value: None,
    },
    Shape {
        case: "a long key name beside a long key value",
        table: "CeilingLongKey",
        partition: "partitionkeyname",
        sort: None,
        partition_value: "0123456789012345",
        sort_value: None,
    },
    Shape {
        case: "a composite key charged across its two attributes",
        table: "CeilingCompositeKey",
        partition: "pk",
        sort: Some("sk"),
        partition_value: "abcd",
        sort_value: Some("efgh"),
    },
];

/// RULE: on every update surface, the largest change an item accepts is the ceiling less what
/// the rest of the item holds and less the part of the key-portion floor its key does not fill
/// — that change is applied whole, what the item it leaves is CHARGED is the ceiling itself,
/// and one byte more is refused with the update-path sentence while the stored item keeps its
/// value.
#[tokio::test]
async fn an_update_tops_out_at_the_floor_bound_item_ceiling() {
    let router = emulator();

    for shape in SHAPES {
        create_active_table(&router, shape.table, &shape.key_schema()).await;
        for surface in SURFACES {
            let surface = *surface;
            let key = shape.key_for(surface);
            let mut seed = key.clone();
            seed[WRITTEN] = json!({"S": SEEDED});
            ok(
                &router,
                "PutItem",
                json!({"TableName": shape.table, "Item": seed}),
            )
            .await;

            // The key portion of THIS item: the surface's suffix makes its value one byte
            // longer than the shape's own.
            let key_bytes = shape.key_bytes() + surface.suffix().len();
            // The key portion is charged at its floor where the change is stated as an
            // expression and it holds less than that, so the headroom the written attribute
            // has is the ceiling less what the key is CHARGED on this surface.
            let key_charge = if surface.charges_the_key_floor() {
                key_bytes.max(KEY_PORTION_FLOOR)
            } else {
                key_bytes
            };
            let largest = MAX_ITEM_BYTES - key_charge - WRITTEN.len();
            let case = format!("{} on {}", surface.operation(), shape.case);

            // At the boundary the change is applied...
            let answered = call(
                &router,
                surface.operation(),
                surface.request(shape, &key, largest),
            )
            .await;
            assert_served(&case, surface, &answered, largest, key_bytes);
            let stored = stored_length(&router, shape, &key).await;
            assert_eq!(
                stored,
                Some(largest),
                "case {case:?}: expected the whole operand to be stored, got {stored:?} byte(s)"
            );
            // ...and what the item it leaves is CHARGED is the ceiling itself: its non-key
            // bytes plus what its key portion is charged.
            let charged = key_charge + WRITTEN.len() + largest;
            assert_eq!(
                charged, MAX_ITEM_BYTES,
                "case {case:?}: expected the stored item to top out at the ceiling over its \
                 {key_bytes}-byte key charged as {key_charge}, got {charged}"
            );

            // One byte past it the change is refused, with the item left as it was.
            let answered = call(
                &router,
                surface.operation(),
                surface.request(shape, &key, largest + 1),
            )
            .await;
            assert_refused(&case, surface, &answered);
            let stored = stored_length(&router, shape, &key).await;
            assert_eq!(
                stored,
                Some(largest),
                "case {case:?}: a refused change must leave the stored item where it was, got \
                 {stored:?} byte(s)"
            );
        }
    }
}

/// The length of the written attribute as a `ConsistentRead` `GetItem` reports it.
async fn stored_length(
    router: &smithy_server_runtime::MultiServiceRouter,
    shape: &Shape,
    key: &Value,
) -> Option<usize> {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": shape.table, "Key": key, "ConsistentRead": true}),
    )
    .await;
    read["Item"][WRITTEN]["S"].as_str().map(str::len)
}

/// Assert the change at the boundary was SERVED on this surface: every surface but the batch
/// answers 200 with no error, and the batch answers 200 with no error in its slot.
fn assert_served(
    case: &str,
    surface: Surface,
    answered: &(u16, Value),
    operand: usize,
    key_bytes: usize,
) {
    let (status, body) = answered;
    assert_eq!(
        *status, 200,
        "case {case:?}: expected the change at the boundary ({operand} operand bytes over a \
         {key_bytes}-byte key) to be applied, got {status} with {}",
        message(body)
    );
    if surface == Surface::BatchExecuteStatement {
        assert_eq!(
            body["Responses"][0].get("Error"),
            None,
            "case {case:?}: expected the batch's slot to carry no error, got {}",
            body["Responses"][0]
        );
    }
}

/// Assert the change one byte past the boundary did not go through, each surface stating it as
/// that surface does: as the CALL's own refusal with the update path's sentence on the
/// single-item, standalone PartiQL and PartiQL-TRANSACTIONAL surfaces and on
/// `TransactWriteItems` — each row's change writes its whole non-key payload itself, so what
/// crossed the ceiling is what the STATEMENT asks for, which is a fact of the request on every
/// surface — and as the slot's own error in a batch, which answers 200.
fn assert_refused(case: &str, surface: Surface, answered: &(u16, Value)) {
    let (status, body) = answered;
    if surface == Surface::BatchExecuteStatement {
        assert_eq!(
            *status, 200,
            "case {case:?}: a batch answers 200 whatever its slots say, got {status} with {body:?}"
        );
        assert_eq!(
            body["Responses"][0]["Error"]["Message"].as_str(),
            Some(UPDATE_TOO_BIG),
            "case {case:?}: expected the slot to state the update-path ceiling refusal, got {}",
            body["Responses"][0]
        );
        return;
    }
    assert_eq!(
        (*status, message(body).as_str()),
        (400, UPDATE_TOO_BIG),
        "case {case:?}: expected the update-path ceiling refusal one byte past the boundary, \
         got {status}"
    );
}
