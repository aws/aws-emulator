// SPDX-License-Identifier: CC0-1.0

//! The documented item-size quota over a table that carries LOCAL secondary indexes.
//!
//! The rule under test: what an item is charged against the 400 KB (409,600-byte) item quota
//! is the bytes it stores PLUS the entry a local secondary index of its table holds for it —
//! "for each local secondary index on a table, there is a 400 KB limit on the total size of
//! the following: the data in the table item, plus the index key values and projected
//! attributes in the index entry" ("Service, Account, and Table Quotas" in the Amazon
//! DynamoDB Developer Guide). So an item charged exactly 409,600 bytes is stored and reads
//! back whole, and the same item one byte longer is refused 400 `ValidationException` with
//! the sentence the path that crossed it words that quota with, leaving the stored item
//! where it was.
//!
//! The rows span what the rule generalises over:
//!
//! * the index's PROJECTION, which decides what its entry holds — `ALL` (the whole item
//!   again, so the quota bites at half the bytes), `KEYS_ONLY` (the index and primary key
//!   attributes alone) and `INCLUDE` (those plus the attributes it names) — against the
//!   CONTROL of a table with no local index at all, where the quota is the item's own bytes;
//! * how many local indexes the table carries, since the quota is published per index: a
//!   table with a narrow index beside a wide one is held to the WIDEST entry;
//! * the SURFACE the write arrives through: `PutItem`, `BatchWriteItem`, a transaction's
//!   `Put` and a PartiQL `INSERT`, plus the two surfaces that state a CHANGE to the stored
//!   item (`UpdateItem` and a PartiQL `UPDATE`), whose refusal is the update path's sentence;
//! * which side of the boundary the write sits on: at the charged ceiling and one byte past
//!   it.
//!
//! Every byte count below is computed from the published sizing rule ("Item sizes and
//! formats": an item's size is "the sum of the lengths of its attribute names and values"),
//! never written down as one number, so a row states the rule rather than a measurement.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The documented item ceiling, 400 KB ("The maximum item size in DynamoDB is 400 KB").
const MAX_ITEM_BYTES: i64 = 400 * 1024;

/// AWS's fixed sentence for a request that CARRIED an item past that ceiling.
const ITEM_TOO_BIG: &str = "Item size has exceeded the maximum allowed size";

/// AWS's fixed sentence for a CHANGE that would leave the stored item past it.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The local secondary index every indexed row declares.
const INDEX: &str = "by-local-sort";

/// How many bytes each of the three key attributes' VALUES carries. Ten, so the part of the
/// item an update leaves untouched is past the floor the update path charges a short one at
/// (`state::UNTOUCHED_PORTION_FLOOR_BYTES`) and the boundary this suite reads is the quota
/// itself.
const KEY_VALUE_BYTES: usize = 10;

/// The bytes the item below stores BESIDE its payload attribute's VALUE: the three key
/// attributes (`pk`, `sk` and the index's own `ls`, two bytes of name and
/// [`KEY_VALUE_BYTES`] of value each), the one-byte-by-one-byte `w` attribute, and the
/// payload attribute's own one-byte name.
const ITEM_WITHOUT_PAYLOAD: i64 = KEY_ATTRIBUTE_BYTES + INCLUDED_BYTES + 1;

/// The bytes the three KEY attributes store, which is what a `KEYS_ONLY` entry holds: the
/// index's own key (`pk`, `ls`) and the table's (`pk`, `sk`), each attribute counted once.
const KEY_ATTRIBUTE_BYTES: i64 = 3 * (2 + KEY_VALUE_BYTES as i64);

/// The bytes the `w` attribute stores, which an `INCLUDE` index naming it adds to its entry.
const INCLUDED_BYTES: i64 = 1 + 1;

/// One row: a table, the local index it declares (if any), and what that index's entry holds
/// for the item below.
#[derive(Clone)]
struct Row {
    case: &'static str,
    table: &'static str,
    /// The `Projection` the local index is declared with, or `None` for a control table,
    /// which declares no local index at all.
    projection: Option<Value>,
    /// What that index's entry holds BESIDE the payload attribute's value.
    entry_without_payload: i64,
    /// Whether the index projects the payload attribute too, so its value is charged twice.
    projects_payload: bool,
}

impl Row {
    /// The payload length at which this table charges the item exactly [`MAX_ITEM_BYTES`]:
    /// the bytes the item stores plus the bytes its index entry stores, solved for the
    /// payload.
    fn payload_at_the_ceiling(&self) -> usize {
        let fixed = ITEM_WITHOUT_PAYLOAD + self.entry_without_payload;
        let per_payload_byte = 1 + i64::from(self.projects_payload);
        usize::try_from((MAX_ITEM_BYTES - fixed) / per_payload_byte)
            .expect("the ceiling leaves room for a payload")
    }

    /// The same row against another table, so one projection can be driven through several
    /// surfaces without two tables of one name.
    fn against(&self, table: &'static str) -> Self {
        Self {
            table,
            ..self.clone()
        }
    }
}

/// The `ALL`-projected row: its entry is the whole item again, so the quota bites at half
/// the bytes. Named because the surface and change rows are driven against it.
fn all_projected() -> Row {
    Row {
        case: "an ALL-projected local index, whose entry is the item again",
        table: "LsiQuotaAll",
        projection: Some(json!({"ProjectionType": "ALL"})),
        entry_without_payload: ITEM_WITHOUT_PAYLOAD,
        projects_payload: true,
    }
}

/// The CONTROL row: no local index, so the quota is the item's own bytes.
fn unindexed() -> Row {
    Row {
        case: "no local index at all, where the quota is the item's own bytes",
        table: "LsiQuotaNone",
        projection: None,
        entry_without_payload: 0,
        projects_payload: false,
    }
}

/// Every projection the published quota counts differently, beside that control.
fn rows() -> Vec<Row> {
    vec![
        all_projected(),
        Row {
            case: "a KEYS_ONLY local index, whose entry is the key attributes alone",
            table: "LsiQuotaKeysOnly",
            projection: Some(json!({"ProjectionType": "KEYS_ONLY"})),
            entry_without_payload: KEY_ATTRIBUTE_BYTES,
            projects_payload: false,
        },
        Row {
            case: "an INCLUDE local index projecting one further attribute",
            table: "LsiQuotaInclude",
            projection: Some(json!({"ProjectionType": "INCLUDE", "NonKeyAttributes": ["w"]})),
            entry_without_payload: KEY_ATTRIBUTE_BYTES + INCLUDED_BYTES,
            projects_payload: false,
        },
        unindexed(),
    ]
}

/// Create `row`'s table — with its local secondary index where it declares one — and poll
/// until it reports `ACTIVE`, which is the waiter a client runs after a create.
async fn created(router: &MultiServiceRouter, row: &Row) {
    let mut definitions = vec![
        json!({"AttributeName": "pk", "AttributeType": "S"}),
        json!({"AttributeName": "sk", "AttributeType": "S"}),
    ];
    let mut request = json!({
        "TableName": row.table,
        "KeySchema": [
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "sk", "KeyType": "RANGE"},
        ],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(projection) = row.projection.clone() {
        // The index's sort key is a key attribute of the table, so it carries a definition;
        // on the control table `ls` is an ordinary attribute and needs none.
        definitions.push(json!({"AttributeName": "ls", "AttributeType": "S"}));
        request["LocalSecondaryIndexes"] = json!([{
            "IndexName": INDEX,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "ls", "KeyType": "RANGE"},
            ],
            "Projection": projection,
        }]);
    }
    request["AttributeDefinitions"] = Value::Array(definitions);
    ok(router, "CreateTable", request).await;
    describe_when_active(router, row.case, row.table).await;
}

/// The item every row writes: the three key attributes at [`KEY_VALUE_BYTES`] of value each,
/// the one-byte `w` attribute, and a payload of `payload` single-byte characters.
fn item(key: &str, payload: usize) -> Value {
    json!({
        "pk": {"S": padded(key)},
        "sk": {"S": padded("sort")},
        "ls": {"S": padded("local")},
        "w": {"S": "z"},
        "v": {"S": "x".repeat(payload)},
    })
}

/// The primary key of [`item`].
fn key_of(key: &str) -> Value {
    json!({"pk": {"S": padded(key)}, "sk": {"S": padded("sort")}})
}

/// `text` as a [`KEY_VALUE_BYTES`]-byte key value, so every key attribute is the same
/// documented length whatever a row calls it. A longer `text` would be two keys collapsed
/// into one, so it is refused rather than cut.
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

/// The payload the stored item at `key` carries, or `None` for a key that holds no item.
async fn stored_payload(router: &MultiServiceRouter, table: &str, key: &str) -> Option<usize> {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": key_of(key), "ConsistentRead": true}),
    )
    .await;
    read["Item"]["v"]["S"].as_str().map(str::len)
}

#[tokio::test]
async fn the_widest_local_index_entry_is_the_one_that_decides() {
    let router = emulator();
    // The quota is published PER local secondary index, so a table carrying several is held
    // to the one whose entry is widest: this table declares a `KEYS_ONLY` index beside an
    // `ALL`-projected one, and the boundary is the `ALL` one's.
    let table = "LsiQuotaWidest";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "ls", "AttributeType": "S"},
                {"AttributeName": "w", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "narrow",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "w", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
                {
                    "IndexName": INDEX,
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "ls", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    describe_when_active(&router, "two local indexes", table).await;

    let at_the_ceiling = all_projected().payload_at_the_ceiling();
    let (status, body) = call(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": item("admitted", at_the_ceiling)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"two local indexes, at the widest entry's ceiling\": the item is stored, got \
         {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "PutItem",
        json!({"TableName": table, "Item": item("refused", at_the_ceiling + 1)}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), message(&body).as_str()),
        (400, "ValidationException", ITEM_TOO_BIG),
        "case \"two local indexes, one byte past it\": expected the quota refusal, got \
         {status} with {body}"
    );
    let stored = stored_payload(&router, table, "refused").await;
    assert_eq!(
        stored, None,
        "case \"two local indexes, one byte past it\": the refused write stored nothing, got \
         {stored:?}"
    );
}

#[tokio::test]
async fn an_item_is_charged_the_local_index_entry_it_would_add() {
    let router = emulator();

    for row in rows() {
        created(&router, &row).await;
        let at_the_ceiling = row.payload_at_the_ceiling();
        let case = row.case;

        // At the charged ceiling the write is served and the item reads back whole.
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": row.table, "Item": item("admitted", at_the_ceiling)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an item charged exactly {MAX_ITEM_BYTES} bytes is stored, got \
             {status} with {body}"
        );
        let stored = stored_payload(&router, row.table, "admitted").await;
        assert_eq!(
            stored,
            Some(at_the_ceiling),
            "case {case:?}: the stored item carries the payload it was written with, got \
             {stored:?}"
        );

        // One byte past it the call is refused with the carried-item sentence, and the
        // stored item is the one the accepted write left.
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": row.table, "Item": item("admitted", at_the_ceiling + 1)}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", ITEM_TOO_BIG),
            "case {case:?}: one byte past the charged ceiling is refused, got {status} with \
             {body}"
        );
        let stored = stored_payload(&router, row.table, "admitted").await;
        assert_eq!(
            stored,
            Some(at_the_ceiling),
            "case {case:?}: the refused write left the stored item where it was, got {stored:?}"
        );
    }
}

/// One write surface: how it carries an item of a stated payload length into a stated table.
type Surface = (
    &'static str,
    &'static str,
    fn(&str, &str, usize) -> (&'static str, Value),
);

#[tokio::test]
async fn every_write_surface_charges_the_same_local_index_entry() {
    let router = emulator();
    // Driven against the `ALL`-projected index, which is where the entry moves the boundary
    // furthest: the surfaces must agree with the single-item path's boundary to the byte.
    let row = all_projected().against("LsiQuotaSurfaces");
    created(&router, &row).await;
    let at_the_ceiling = row.payload_at_the_ceiling();

    // Each row: the case, the key the admitted write stores (short enough that the key the
    // refused write below addresses is a DIFFERENT one once padded), and the request that
    // writes a payload of the length it is given.
    let surfaces: Vec<Surface> = vec![
        ("a batched write", "bat", |table, key, payload| {
            (
                "BatchWriteItem",
                json!({"RequestItems": {table: [{"PutRequest": {"Item": item(key, payload)}}]}}),
            )
        }),
        ("a transactional put", "txn", |table, key, payload| {
            (
                "TransactWriteItems",
                json!({"TransactItems": [{"Put": {
                    "TableName": table, "Item": item(key, payload)}}]}),
            )
        }),
        ("a PartiQL insert", "sql", |table, key, payload| {
            (
                "ExecuteStatement",
                json!({
                    "Statement": format!(
                        "INSERT INTO \"{table}\" VALUE {{'pk': '{}', 'sk': '{}', \
                         'ls': '{}', 'w': 'z', 'v': ?}}",
                        padded(key),
                        padded("sort"),
                        padded("local")
                    ),
                    "Parameters": [{"S": "x".repeat(payload)}],
                }),
            )
        }),
    ];

    for (case, key, request) in surfaces {
        let (operation, admitted) = request(row.table, key, at_the_ceiling);
        let (status, body) = call(&router, operation, admitted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an item charged exactly {MAX_ITEM_BYTES} bytes is stored, got \
             {status} with {body}"
        );
        let stored = stored_payload(&router, row.table, key).await;
        assert_eq!(
            stored,
            Some(at_the_ceiling),
            "case {case:?}: the stored item carries the payload it was written with, got \
             {stored:?}"
        );

        // One byte past the ceiling, under a key of its own so what is asserted is the
        // refusal and not a replacement: the call is refused and that key holds nothing.
        let past = format!("{key}-past");
        let (operation, oversized) = request(row.table, &past, at_the_ceiling + 1);
        let (status, body) = call(&router, operation, oversized).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", ITEM_TOO_BIG),
            "case {case:?}: one byte past the charged ceiling is refused, got {status} with \
             {body}"
        );
        let stored = stored_payload(&router, row.table, &past).await;
        assert_eq!(
            stored, None,
            "case {case:?}: the refused write stored nothing, got {stored:?}"
        );
    }
}

/// One change surface: how it grows the stored item of a stated table to a stated payload.
type Change = (&'static str, fn(&str, usize) -> (&'static str, Value));

#[tokio::test]
async fn a_change_is_charged_the_local_index_entry_the_item_it_leaves_would_add() {
    let router = emulator();

    // The two tables a change is driven against: the `ALL`-projected one, whose entry is the
    // item again, and the control with no local index — so what the rows differ in is the
    // entry the quota counts beside the item the change leaves.
    for row in [
        all_projected().against("LsiQuotaChangedAll"),
        unindexed().against("LsiQuotaChangedNone"),
    ] {
        created(&router, &row).await;
        let at_the_ceiling = row.payload_at_the_ceiling();
        let case = row.case;

        // The item is seeded with a one-byte payload and then GROWN by a change, so what the
        // quota is charged over is the item the change LEAVES rather than the bytes the
        // request carried.
        ok(
            &router,
            "PutItem",
            json!({"TableName": row.table, "Item": item("changed", 1)}),
        )
        .await;

        // Each row: the case and how the change is stated — an `UpdateExpression` and a
        // PartiQL `UPDATE`, the two surfaces that state a change to one item.
        let changes: Vec<Change> = vec![
            ("an update expression", |table, payload| {
                (
                    "UpdateItem",
                    json!({"TableName": table, "Key": key_of("changed"),
                           "UpdateExpression": "SET v = :v",
                           "ExpressionAttributeValues": {":v": {"S": "x".repeat(payload)}}}),
                )
            }),
            ("a PartiQL update", |table, payload| {
                (
                    "ExecuteStatement",
                    json!({
                        "Statement": format!(
                            "UPDATE \"{table}\" SET v = ? WHERE pk = '{}' AND sk = '{}'",
                            padded("changed"),
                            padded("sort")
                        ),
                        "Parameters": [{"S": "x".repeat(payload)}],
                    }),
                )
            }),
        ];

        for (surface, change) in changes {
            let (operation, admitted) = change(row.table, at_the_ceiling);
            let (status, body) = call(&router, operation, admitted).await;
            assert_eq!(
                status, 200,
                "case {case:?} through {surface:?}: a change leaving the item charged exactly \
                 {MAX_ITEM_BYTES} bytes is applied, got {status} with {body}"
            );
            let (operation, oversized) = change(row.table, at_the_ceiling + 1);
            let (status, body) = call(&router, operation, oversized).await;
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, "ValidationException", UPDATE_TOO_BIG),
                "case {case:?} through {surface:?}: one byte past the charged ceiling is \
                 refused with the update path's sentence, got {status} with {body}"
            );
            let stored = stored_payload(&router, row.table, "changed").await;
            assert_eq!(
                stored,
                Some(at_the_ceiling),
                "case {case:?} through {surface:?}: the refused change left the item the \
                 accepted one applied, got {stored:?}"
            );
        }
    }
}
