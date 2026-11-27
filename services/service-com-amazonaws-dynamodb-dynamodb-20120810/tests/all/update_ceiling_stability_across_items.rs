// SPDX-License-Identifier: CC0-1.0

//! Where an UPDATE's item-size refusal starts, asked of SEVERAL items: the same request
//! shape is refused at the same operand length on every one of them.
//!
//! The rule under test: what an update is charged against the documented 400 KB item ceiling
//! is the bytes the item it leaves behind stores — the part of it the change does not address,
//! its key attributes among them, charged at a minimum of its own,
//! `state::UNTOUCHED_PORTION_FLOOR_BYTES` — plus a frame per clause the request states.
//! Nothing else enters it, so the SMALLEST operand length one request shape is refused at is a
//! property of that shape and of the item's non-key bytes alone: it does not move from one
//! item or table to the next because one item's primary key holds fewer bytes than the other's
//! (while both hold less than the floor). A client that probes its own payload budget against
//! one item and then writes the next relies on exactly that, and the refusal is silent in the
//! accepted direction, so a boundary that moved would surface only later, as items real AWS
//! could not hold.
//!
//! How each row is driven, which is how such a boundary is discovered over the wire:
//!
//! 1. the smallest refused operand length is MEASURED on a reference table — a binary search
//!    between a length the service admits and one it refuses, then re-verified from both
//!    sides — so every assertion below is about the service's own boundary rather than about
//!    this suite's arithmetic. Each candidate is probed on a FRESH single-attribute item of
//!    that table, keyed by its own eight-character value, so no probe reads what another
//!    probe left and none of them shares a partition key's write budget; and
//! 2. that same length is re-asserted on a freshly created table whose item was written with
//!    its hash key alone, keyed by a DIFFERENT number of bytes — the control. It must be
//!    refused there too, with the update path's fixed sentence, and must leave the item
//!    exactly as it was; one byte under it must still be applied, so the control's boundary is
//!    pinned from both sides rather than merely "refused eventually".
//!
//! The rows span what the rule generalises over: the clause kinds whose frames differ (one
//! plain assignment, two assignments, an assignment whose operand is a function call, one
//! stated beside arithmetic, one beside an `ADD`, one beside a `REMOVE`) crossed with key
//! portions of ten bytes (the reference), nine and two — the nine- and ten-byte pair differing
//! by the single byte that must move nothing.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB
//! Developer Guide ("The maximum item size in DynamoDB is 400 KB"); `UpdateItem` declares no
//! size error of its own, so a change past it is the service-common `ValidationException`.
//! `update_item_ceiling_key_portion.rs` asserts where the ceiling SITS as the key portion
//! varies and `request_shape_validation_classes.rs` what each clause kind costs; this suite is
//! about the boundary being the SAME on two items of the same shape.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::UNTOUCHED_PORTION_FLOOR_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The documented item ceiling, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// How far under the ceiling the search for a boundary starts. A boundary sits at the ceiling
/// less what the item's key portion is charged and what the stated clauses cost, both of which
/// are tens of bytes, so a window this wide brackets every row with room to spare — and the
/// search asserts both of its ends, so a row whose boundary fell outside it would say so.
const SEARCH_WINDOW: usize = 512;

/// AWS's fixed sentence for a change that would grow the stored item past the ceiling. The
/// WHOLE message: it carries no value from the request.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The attribute whose operand length every row varies.
const WRITTEN: &str = "v";

/// The hash-key attribute of the table each row measures its boundary on. Its items are keyed
/// by eight-character values, so the key portion there holds ten bytes
/// ([`REFERENCE_KEY_BYTES`]).
const REFERENCE_KEY: &str = "pk";

/// How many bytes each key value a measured row writes holds, which with the attribute's own
/// name is that row's key portion.
const REFERENCE_KEY_VALUE_BYTES: usize = 8;

/// The bytes the reference row's key portion holds: the attribute's name and its value.
const REFERENCE_KEY_BYTES: usize = REFERENCE_KEY.len() + REFERENCE_KEY_VALUE_BYTES;

/// One control item: the table's hash-key attribute and the single value its only item is held
/// under. Both hold fewer bytes than the key portion's floor, as the reference does, so all
/// three must share one boundary.
struct KeyShape {
    case: &'static str,
    suffix: &'static str,
    attribute: &'static str,
    value: &'static str,
}

impl KeyShape {
    /// The bytes this control's key portion holds: the key attribute's name and its value.
    fn key_bytes(&self) -> usize {
        self.attribute.len() + self.value.len()
    }

    /// The `Key` map this control's item is addressed by — which is also the WHOLE item until
    /// a change is applied to it.
    fn key(&self) -> Value {
        key_of(self.attribute, self.value)
    }
}

/// The controls each measured boundary is re-asserted on: nine bytes of key portion (one byte
/// less than the reference) and two (the fewest a table can be keyed by).
const CONTROLS: &[KeyShape] = &[
    KeyShape {
        case: "a nine-byte key portion, one byte less than the reference",
        suffix: "Nine",
        attribute: "pk",
        value: "refused",
    },
    KeyShape {
        case: "a two-byte key portion",
        suffix: "Two",
        attribute: "k",
        value: "1",
    },
];

/// One request shape: the clauses it states, and the values it binds beside `:v`, the operand
/// whose length is varied.
struct Shape {
    case: &'static str,
    prefix: &'static str,
    expression: &'static str,
    /// `(placeholder, AttributeValue member, value)` for every binding other than `:v`.
    bound: &'static [(&'static str, &'static str, &'static str)],
}

/// The clause kinds whose frames differ. Each states ONE assignment whose operand is the
/// length being varied, so what distinguishes the rows is what is stated beside it; every
/// other clause writes a value that does not change from probe to probe, so the item a probe
/// leaves behind depends on the operand length alone.
const SHAPES: &[Shape] = &[
    Shape {
        case: "a plain assignment",
        prefix: "CeilPlain",
        expression: "SET v = :v",
        bound: &[],
    },
    Shape {
        case: "an assignment beside a second assignment",
        prefix: "CeilTwoSets",
        expression: "SET v = :v, w = :w",
        bound: &[(":w", "S", "y")],
    },
    Shape {
        case: "an assignment whose operand is a function call",
        prefix: "CeilFunction",
        expression: "SET v = if_not_exists(q, :v)",
        bound: &[],
    },
    Shape {
        case: "an assignment beside an arithmetic one",
        prefix: "CeilArithmetic",
        expression: "SET v = :v, n = :one + :two",
        bound: &[(":one", "N", "1"), (":two", "N", "2")],
    },
    Shape {
        case: "an assignment beside an ADD",
        prefix: "CeilAdd",
        expression: "SET v = :v ADD n :zero",
        bound: &[(":zero", "N", "0")],
    },
    Shape {
        case: "an assignment beside a REMOVE",
        prefix: "CeilRemove",
        expression: "SET v = :v REMOVE q",
        bound: &[],
    },
];

/// RULE: the smallest operand length one request shape is refused at is the same on every item
/// whose key portion holds less than the floor — so a boundary measured on one table holds on
/// the next, where the refusal carries the update path's fixed sentence and leaves the item
/// untouched, and one byte under it is still applied.
#[tokio::test]
async fn the_refusal_boundary_of_a_request_shape_does_not_move_between_items() {
    let router = emulator();

    // One ceiling is shared by the items whose untouched part holds LESS than the floor that
    // part is charged at, so every row of this suite has to lie in that domain: a row that
    // left it says so here rather than reporting a boundary that moved. Every item here holds
    // its key alone before the change, so its key portion IS that untouched part.
    let floor = usize::try_from(UNTOUCHED_PORTION_FLOOR_BYTES)
        .expect("the floor the untouched part of an item is charged at is a byte count");
    let in_domain = std::iter::once(("the reference", REFERENCE_KEY_BYTES)).chain(
        CONTROLS
            .iter()
            .map(|control| (control.case, control.key_bytes())),
    );
    for (case, key_bytes) in in_domain {
        assert!(
            key_bytes < floor,
            "case {case:?}: this suite's rows are items whose key portion holds less than the \
             {floor} bytes it is charged at, got {key_bytes}"
        );
    }

    // Every table this suite drives is created first and waited for afterwards: the clock owes
    // each new table the same transient span, and created together they run it down together.
    for shape in SHAPES {
        create_table(&router, &measured_table(shape), REFERENCE_KEY).await;
        for control in CONTROLS {
            create_table(&router, &control_table(shape, control), control.attribute).await;
        }
    }
    for shape in SHAPES {
        describe_when_active(&router, shape.case, &measured_table(shape)).await;
        for control in CONTROLS {
            let table = control_table(shape, control);
            let case = row(shape, control);
            describe_when_active(&router, &case, &table).await;
            // The item every control starts from: its hash key and nothing else.
            put_item(&router, &table, &control.key()).await;
        }
    }

    for shape in SHAPES {
        let refused_at = smallest_refused_operand(&router, shape).await;

        for control in CONTROLS {
            let table = control_table(shape, control);
            let case = row(shape, control);

            // The boundary measured on the reference items, re-asserted on this one: the same
            // refusal, with the same fixed sentence, and nothing of the request left behind.
            let refusal = update(&table, &control.key(), shape, refused_at);
            let (status, body) = call(&router, "UpdateItem", refusal).await;
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, "ValidationException", UPDATE_TOO_BIG),
                "case {case:?}: a {refused_at}-byte operand is refused on the reference item, so \
                 it must be refused here too, got {status} with {body}"
            );
            let stored = stored_item(&router, &case, &table, &control.key()).await;
            assert_eq!(
                stored,
                control.key(),
                "case {case:?}: a refused update leaves the item as it was — its key alone — got \
                 {stored}"
            );

            // And one byte under it is still applied here, so this item's boundary is the
            // measured one and not merely somewhere at or below it.
            let admitted = refused_at - 1;
            let change = update(&table, &control.key(), shape, admitted);
            let (status, body) = call(&router, "UpdateItem", change).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a {admitted}-byte operand is applied to the reference item, so it \
                 must be applied here too, got {status} with {}",
                message(&body)
            );
            let stored = stored_item(&router, &case, &table, &control.key()).await;
            let length = stored[WRITTEN]["S"].as_str().map(str::len);
            assert_eq!(
                length,
                Some(admitted),
                "case {case:?}: the applied update must leave a {admitted}-byte operand stored, \
                 got {length:?} byte(s)"
            );
        }
    }
}

/// The smallest operand length this request shape is refused at, MEASURED: a binary search
/// between a length the service admits and one it refuses, bracketed by [`SEARCH_WINDOW`]
/// under the ceiling, then re-verified from both sides.
///
/// Every probe is held to one of two outcomes — applied, or refused with the update path's
/// ceiling sentence — so a probe that failed for any other reason stops the search instead of
/// being read as a boundary.
async fn smallest_refused_operand(router: &MultiServiceRouter, shape: &Shape) -> usize {
    let mut probes = Probes::over(router, shape);
    let mut admitted = MAX_ITEM_BYTES - SEARCH_WINDOW;
    let mut refused = MAX_ITEM_BYTES;
    assert!(
        probes.applied(admitted).await,
        "case {:?}: an operand {SEARCH_WINDOW} byte(s) under the ceiling must be applied, so the \
         boundary lies inside the searched window",
        shape.case
    );
    assert!(
        !probes.applied(refused).await,
        "case {:?}: an operand as long as the whole ceiling must be refused",
        shape.case
    );
    while refused - admitted > 1 {
        let probe = admitted + (refused - admitted) / 2;
        if probes.applied(probe).await {
            admitted = probe;
        } else {
            refused = probe;
        }
    }
    // Both sides of the boundary, read off the service again after the search walked past them.
    assert!(
        probes.applied(refused - 1).await,
        "case {:?}: the byte under the boundary {refused} must still be applied",
        shape.case
    );
    assert!(
        !probes.applied(refused).await,
        "case {:?}: the boundary {refused} must be refused",
        shape.case
    );
    refused
}

/// The search's probes for one request shape: each one writes a fresh single-attribute item on
/// that shape's reference table and states the shape's clauses over the candidate operand.
struct Probes<'a> {
    router: &'a MultiServiceRouter,
    shape: &'a Shape,
    table: String,
    /// How many items this search has written, which is what the next one is keyed by — a key
    /// of its own, so a probe neither reads nor is bounded by what an earlier probe left.
    written: u32,
}

impl<'a> Probes<'a> {
    fn over(router: &'a MultiServiceRouter, shape: &'a Shape) -> Self {
        Self {
            router,
            shape,
            table: measured_table(shape),
            written: 0,
        }
    }

    /// Whether an `operand`-byte operand is APPLIED to a fresh item through this shape's
    /// clauses. A refusal must be the update path's ceiling refusal; anything else panics
    /// naming the row.
    async fn applied(&mut self, operand: usize) -> bool {
        self.written += 1;
        // Eight characters, so every probe's key portion holds the same bytes
        // ([`REFERENCE_KEY_BYTES`]).
        let value = format!("{:0width$}", self.written, width = REFERENCE_KEY_VALUE_BYTES);
        let key = key_of(REFERENCE_KEY, &value);
        put_item(self.router, &self.table, &key).await;
        let (status, body) = call(
            self.router,
            "UpdateItem",
            update(&self.table, &key, self.shape, operand),
        )
        .await;
        if status == 200 {
            return true;
        }
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", UPDATE_TOO_BIG),
            "case {:?}: a {operand}-byte operand must be either applied or refused with the \
             update path's ceiling sentence, got {status} with {body}",
            self.shape.case
        );
        false
    }
}

/// The `UpdateItem` request this shape states over `operand` single-byte characters.
fn update(table: &str, key: &Value, shape: &Shape, operand: usize) -> Value {
    let mut values = json!({":v": {"S": "x".repeat(operand)}});
    for (placeholder, member, value) in shape.bound {
        values[*placeholder] = json!({*member: *value});
    }
    json!({
        "TableName": table,
        "Key": key,
        "UpdateExpression": shape.expression,
        "ExpressionAttributeValues": values,
    })
}

/// Write one item holding nothing but the key it is addressed by.
async fn put_item(router: &MultiServiceRouter, table: &str, key: &Value) {
    ok(router, "PutItem", json!({"TableName": table, "Item": key})).await;
}

/// The item a `ConsistentRead` `GetItem` reports for `key` — the table's own account of what
/// it stores, which is what a refused update must have left alone.
async fn stored_item(router: &MultiServiceRouter, case: &str, table: &str, key: &Value) -> Value {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table, "Key": key, "ConsistentRead": true}),
    )
    .await;
    read.get("Item").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: the item written into {table} must still be readable, got {read}")
    })
}

/// Create one on-demand table keyed on the string attribute `attribute`, WITHOUT waiting for
/// it: every table of this suite is created first and polled afterwards, so the transient span
/// the clock owes each of them runs down once rather than once per table — which is why the
/// crate's shared `table_items::create_active_table`, which creates and waits in one, is not
/// what this suite calls. The wait is the harness's own bounded poll
/// ([`describe_when_active`]).
async fn create_table(router: &MultiServiceRouter, table: &str, attribute: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": [{"AttributeName": attribute, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": attribute, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
}

/// The table one request shape measures its boundary on.
fn measured_table(shape: &Shape) -> String {
    format!("{}Measured", shape.prefix)
}

/// The table one (request shape, control item) row re-asserts the boundary on: one table each,
/// so no row's writes are read by another.
fn control_table(shape: &Shape, control: &KeyShape) -> String {
    format!("{}{}", shape.prefix, control.suffix)
}

/// How a (request shape, control item) row names itself in an assertion.
fn row(shape: &Shape, control: &KeyShape) -> String {
    format!("{} on {}", shape.case, control.case)
}

/// The `Key` map addressing the item `attribute` holds `value` for.
fn key_of(attribute: &str, value: &str) -> Value {
    json!({attribute: {"S": value}})
}
