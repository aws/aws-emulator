// SPDX-License-Identifier: CC0-1.0

//! The item-size boundary a change to a stored item is held to, per SURFACE.
//!
//! The rule under test: a change is refused when what the item it would leave behind is
//! CHARGED crosses the documented 400 KB (409,600-byte) item ceiling, charged over the bytes
//! the item stores — "the lengths of its attribute names and values" ("Item sizes and
//! formats"), its key attributes among them — plus what the SHAPE of the request stating the
//! change costs: a change stated as an `UpdateExpression` is charged the part of the item it
//! does not address — here its primary key — at a minimum of
//! `state::UNTOUCHED_PORTION_FLOOR_BYTES` whatever it actually holds, while one
//! stated without an expression is charged the item alone. So the item charged exactly
//! 409,600 bytes is written and read back whole, and the same change one byte longer is
//! refused 400 `ValidationException` with the sentence the update path words that quota with,
//! leaving the stored item where it was.
//!
//! The rows span what the rule generalises over: the SURFACE the change is stated through —
//! an `UpdateExpression`, the legacy `AttributeUpdates` map, and a PartiQL `UPDATE` — crossed
//! with how long the item's key portion is, below the key floor and past it. A PartiQL
//! `INSERT`, which carries a whole item rather than a change to one, is driven beside them at
//! the FLAT ceiling a carried item is held to, which has no key floor of its own.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB
//! Developer Guide ("The maximum item size in DynamoDB is 400 KB"); the floor under the part
//! of the item a change does not address is measured behavior
//! (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).

use serde_json::{json, Value};

use crate::emulator::{call, emulator, message, ok};
use crate::table_items::create_active_table;

/// The documented item ceiling, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// The least an item's primary-key portion is charged against that ceiling when the change
/// is stated as an expression and the key is all it leaves untouched, as the service states it
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`).
const KEY_PORTION_FLOOR: usize = 22;

/// AWS's fixed sentence for a CHANGE that would leave the item past the ceiling.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// AWS's fixed sentence for a request that CARRIES an item past the ceiling.
const ITEM_TOO_BIG: &str = "Item size has exceeded the maximum allowed size";

/// The attribute every row writes its bytes under, one byte of name.
const WRITTEN: &str = "v";

/// How a row states its change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    /// `UpdateItem` with an `UpdateExpression`.
    Expression,
    /// `UpdateItem` with the legacy `AttributeUpdates` map, which states no expression at
    /// all.
    Legacy,
    /// A PartiQL `UPDATE` through `ExecuteStatement`.
    Partiql,
}

impl Surface {
    fn operation(self) -> &'static str {
        match self {
            Surface::Expression | Surface::Legacy => "UpdateItem",
            Surface::Partiql => "ExecuteStatement",
        }
    }

    /// The suffix a row's key value carries on this surface, so each surface writes its own
    /// item.
    fn suffix(self) -> &'static str {
        match self {
            Surface::Expression => "e",
            Surface::Legacy => "l",
            Surface::Partiql => "q",
        }
    }

    /// The request that writes `operand` single-byte characters into `key`'s item.
    fn request(self, table: &str, key_attribute: &str, key_value: &str, operand: usize) -> Value {
        let value = json!({"S": "x".repeat(operand)});
        match self {
            Surface::Expression => json!({
                "TableName": table,
                "Key": {key_attribute: {"S": key_value}},
                "UpdateExpression": format!("SET {WRITTEN} = :v"),
                "ExpressionAttributeValues": {":v": value},
            }),
            Surface::Legacy => json!({
                "TableName": table,
                "Key": {key_attribute: {"S": key_value}},
                "AttributeUpdates": {WRITTEN: {"Action": "PUT", "Value": value}},
            }),
            Surface::Partiql => json!({
                "Statement": format!(
                    "UPDATE \"{table}\" SET {WRITTEN} = ? WHERE {key_attribute} = '{key_value}'"
                ),
                "Parameters": [value],
            }),
        }
    }
}

/// Every surface a change is stated through.
const SURFACES: &[Surface] = &[Surface::Expression, Surface::Legacy, Surface::Partiql];

/// One key shape: the key attribute's name and the value a row's item is addressed by.
struct Shape {
    case: &'static str,
    table: &'static str,
    attribute: &'static str,
    value: &'static str,
}

/// Key portions of three different lengths, which is what moves the boundary.
const SHAPES: &[Shape] = &[
    Shape {
        case: "a one-byte key name and value",
        table: "SizeBoundaryTiny",
        attribute: "k",
        value: "1",
    },
    Shape {
        case: "a nine-byte key portion",
        table: "SizeBoundaryShort",
        attribute: "pk",
        value: "boundar",
    },
    Shape {
        case: "a key portion of thirty-four bytes",
        table: "SizeBoundaryLong",
        attribute: "partitionkeyname",
        value: "0123456789abcdef1",
    },
];

/// The length of the written attribute as a `ConsistentRead` `GetItem` reports it.
async fn stored_length(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    key_attribute: &str,
    key_value: &str,
) -> Option<usize> {
    let read = ok(
        router,
        "GetItem",
        json!({
            "TableName": table,
            "Key": {key_attribute: {"S": key_value}},
            "ConsistentRead": true,
        }),
    )
    .await;
    read["Item"][WRITTEN]["S"].as_str().map(str::len)
}

/// RULE: on every surface a change is stated through, the item charged exactly the documented
/// ceiling is stored and the same change one byte longer is refused — with the key portion
/// charged at its floor, so a key below that floor buys no headroom.
#[tokio::test]
async fn a_change_tops_out_at_the_documented_item_ceiling_on_every_surface() {
    let router = emulator();

    for shape in SHAPES {
        create_active_table(&router, shape.table, &[(shape.attribute, "HASH", "S")]).await;
        for surface in SURFACES {
            let surface = *surface;
            let key_value = format!("{}{}", shape.value, surface.suffix());
            let case = format!("{:?} on {}", surface, shape.case);
            ok(
                &router,
                "PutItem",
                json!({
                    "TableName": shape.table,
                    "Item": {shape.attribute: {"S": key_value.clone()}},
                }),
            )
            .await;

            // The bytes the item is charged besides the operand: the written attribute's
            // name, and what the key portion is charged — its own bytes, raised to the
            // floor while the change is stated as an expression and they do not fill it.
            let key_bytes = shape.attribute.len() + key_value.len();
            let key_charge = match surface {
                Surface::Expression => key_bytes.max(KEY_PORTION_FLOOR),
                Surface::Legacy | Surface::Partiql => key_bytes,
            };
            let fixed = key_charge + WRITTEN.len();
            let at_the_ceiling = MAX_ITEM_BYTES - fixed;

            let (status, body) = call(
                &router,
                surface.operation(),
                surface.request(shape.table, shape.attribute, &key_value, at_the_ceiling),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the item charged {MAX_ITEM_BYTES} bytes to be \
                 written, got {status} with {}",
                message(&body)
            );
            let stored = stored_length(&router, shape.table, shape.attribute, &key_value).await;
            assert_eq!(
                stored,
                Some(at_the_ceiling),
                "case {case:?}: expected the whole operand to be stored, got {stored:?} byte(s)"
            );

            let (status, body) = call(
                &router,
                surface.operation(),
                surface.request(shape.table, shape.attribute, &key_value, at_the_ceiling + 1),
            )
            .await;
            assert_eq!(
                (status, message(&body).as_str()),
                (400, UPDATE_TOO_BIG),
                "case {case:?}: expected one byte past the ceiling to be refused with the \
                 update path's sentence, got {status} with {body}"
            );
            let stored = stored_length(&router, shape.table, shape.attribute, &key_value).await;
            assert_eq!(
                stored,
                Some(at_the_ceiling),
                "case {case:?}: a refused change must leave the stored item where it was, got \
                 {stored:?} byte(s)"
            );
        }
    }
}

/// RULE: a request that CARRIES a whole item is held to the same ceiling over the same bytes
/// — the item at it is written and one byte more is refused — with the sentence AWS words a
/// carried item's quota with.
#[tokio::test]
async fn a_carried_item_tops_out_at_the_same_ceiling() {
    let router = emulator();
    let table = "SizeBoundaryCarried";
    let key_attribute = "pk";
    create_active_table(&router, table, &[(key_attribute, "HASH", "S")]).await;

    // (case, the operation that carries the item, the key it carries it under)
    let cases: &[(&str, &str, &str)] = &[
        ("PutItem", "PutItem", "put"),
        ("a PartiQL INSERT", "ExecuteStatement", "insert"),
    ];
    for (case, operation, key_value) in cases {
        let fixed = key_attribute.len() + key_value.len() + WRITTEN.len();
        let at_the_ceiling = MAX_ITEM_BYTES - fixed;

        for (label, operand, expected) in [
            ("at the ceiling", at_the_ceiling, None),
            ("one byte past it", at_the_ceiling + 1, Some(ITEM_TOO_BIG)),
        ] {
            let payload = "x".repeat(operand);
            let body = if *operation == "PutItem" {
                json!({
                    "TableName": table,
                    "Item": {key_attribute: {"S": key_value}, WRITTEN: {"S": payload}},
                })
            } else {
                json!({
                    "Statement": format!(
                        "INSERT INTO \"{table}\" VALUE {{'{key_attribute}': ?, '{WRITTEN}': ?}}"
                    ),
                    "Parameters": [{"S": key_value}, {"S": payload}],
                })
            };
            let (status, answered) = call(&router, operation, body).await;
            match expected {
                None => assert_eq!(
                    status, 200,
                    "case {case:?} {label}: expected the item at {MAX_ITEM_BYTES} stored bytes \
                     to be written, got {status} with {}",
                    message(&answered)
                ),
                Some(sentence) => assert_eq!(
                    (status, message(&answered).as_str()),
                    (400, sentence),
                    "case {case:?} {label}: expected the carried-item refusal, got {status} \
                     with {answered}"
                ),
            }
        }

        let stored = stored_length(&router, table, key_attribute, key_value).await;
        assert_eq!(
            stored,
            Some(at_the_ceiling),
            "case {case:?}: expected the accepted item to be the one stored, got {stored:?} \
             byte(s)"
        );
    }
}
