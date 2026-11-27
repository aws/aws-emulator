// SPDX-License-Identifier: CC0-1.0

//! WHICH ANSWER an update that would leave an item past the documented item-size ceiling
//! earns, across the surfaces an update arrives through.
//!
//! The rule under test: the ceiling is the ITEM's and does not move with the surface the
//! change arrived through, so the SENTENCE is the same wherever it was stated — but WHOSE
//! failure the crossing is follows WHICH ceiling the update path held the request to.
//! A SINGLE-ITEM update states a change on its own, so crossing the ceiling is refused as the
//! request it is: HTTP 400, the validation class, and the sentence the update path words that
//! quota with. Inside a `TransactWriteItems` transaction the same two classes split by what the
//! update path CHARGES that request beside the bytes the item it leaves stores: a request it
//! charges a ceiling of its own — the part of the item its clauses leave alone below the floor
//! that part is charged at, a clause frame beside the one assignment every such write carries —
//! was held to a ceiling under the quota that no stored item can lift, so the
//! CALL is refused exactly as the single-item update of that statement is, and no action is
//! graded. A request charged nothing of its own was held to the quota's own count, so a
//! crossing there is "an item size becomes too large (larger than 400 KB) [...] or a similar
//! validation error occurs because of changes made by the transaction" — one of the
//! circumstances `TransactionCanceledException` documents — so the transaction is CANCELLED:
//! HTTP 400, the cancellation class, the top-level sentence over the per-slot codes in request
//! order, and an ordered reason list naming the crossing action with that same sentence while
//! every blameless action reports `None`. `update_item_ceiling_key_portion.rs` and
//! `transactional_update_clause_charge_class.rs` drive each of the two charges on its own.
//! Nothing of a refused or a cancelled call is applied.
//!
//! The rows span what the rule generalises over: the surface a change is stated through
//! (`UpdateItem` and a transaction's `Update` for an `UpdateExpression`, `ExecuteStatement`
//! and `ExecuteTransaction` for a PartiQL `UPDATE`), HOW the ceiling is crossed (one value
//! past it on its own, a value inside it that only crosses together with what the item
//! already holds, two clauses that cross only together, a value the update COMPUTES), whether
//! the key the change addresses already HOLDS an item, and WHERE the oversized action sits in
//! a transaction that carries more than one.
//!
//! The BOUNDARIES are driven beside them: a change inside the ceiling COMMITS through a
//! transaction and reads back, an action whose own condition is not met cancels the transaction
//! with its own reason code rather than the size one, and an item a `Put` CARRIES past the
//! ceiling earns the request refusal for an item that was carried.
//!
//! Source of the ceiling: "Service, Account, and Table Quotas" in the Amazon DynamoDB
//! Developer Guide ("The maximum item size in DynamoDB is 400 KB"). Source of the class the
//! crossing takes inside a transaction: `TransactWriteItems`'s own documented cancellation
//! conditions — "an item size becomes too large (bigger than 400 KB) [...] or a similar
//! validation error occurs because of changes made by the transaction".

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, error_type, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses: one string partition key.
const TABLE: &str = "TxnUpdateCeiling";

/// The documented item ceiling, 400 KB.
const MAX_ITEM_BYTES: usize = 400 * 1024;

/// AWS's fixed sentence for a change that would leave the item past the ceiling.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// AWS's fixed sentence for a request that CARRIED an item past the ceiling.
const ITEM_TOO_BIG: &str = "Item size has exceeded the maximum allowed size";

/// The whole shape id the wire carries for the validation class, which is what a client
/// branching on `__type` matches.
const VALIDATION_SHAPE_ID: &str = "com.amazon.coral.validate#ValidationException";

/// The `CancellationReason$Code` an action that broke a rule of the service reports.
const VALIDATION_ERROR_CODE: &str = "ValidationError";

/// The `CancellationReason$Code` an action nothing was wrong with reports.
const NO_REASON_CODE: &str = "None";

/// The whole top-level sentence a cancelled transaction carries, over the per-slot codes in
/// request order: the sentence is fixed text and the bracketed list is the request's own
/// shape.
fn cancelled_message(codes: &[&str]) -> String {
    format!(
        "Transaction cancelled, please refer cancellation reasons for specific reasons [{}]",
        codes.join(", ")
    )
}

/// An emulator holding the table every row writes into.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    router
}

/// The key map addressing `key`.
fn key_of(key: &str) -> Value {
    json!({"pk": {"S": key}})
}

/// Store an item under `key` carrying `bytes` of filler, which is the state a row's change is
/// then read against.
async fn seed_item(router: &MultiServiceRouter, key: &str, bytes: usize) {
    ok(
        router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": key}, "held": {"S": "h".repeat(bytes)}}}),
    )
    .await;
}

/// What a row's change is refused with, asserted as a client reads it: the status, the whole
/// validation shape id, the sentence, and the absence of a reason list — a refused call grades
/// no action, so a request that is refused carries no `CancellationReasons`.
fn assert_refused(case: &str, status: u16, body: &Value, sentence: &str) {
    assert_eq!(
        (status, error_type(body).as_str(), message(body).as_str()),
        (400, VALIDATION_SHAPE_ID, sentence),
        "case {case:?}: expected 400 {VALIDATION_SHAPE_ID} {sentence:?}, got {status} with {body}"
    );
    assert!(
        body.get("CancellationReasons").is_none(),
        "case {case:?}: a refused call grades no action, so it reports no reason list, got {body}"
    );
}

/// What a cancelled transaction answers, asserted as a client reads it: the status, the
/// cancellation class, the top-level sentence over `codes` in request order, and one reason
/// per action — `messages` is the sentence each action reports, `None` for an action nothing
/// was wrong with.
fn assert_cancelled(
    case: &str,
    status: u16,
    body: &Value,
    codes: &[&str],
    messages: &[Option<&str>],
) {
    assert_eq!(
        (
            status,
            error_code(body).as_str(),
            message(body).as_str()
        ),
        (
            400,
            "TransactionCanceledException",
            cancelled_message(codes).as_str()
        ),
        "case {case:?}: expected the transaction cancelled over {codes:?}, got {status} with \
         {body}"
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
        "case {case:?}: expected the ordered reason list {expected:?}, got {reported:?} in {body}"
    );
}

/// The length of the filler the item under `key` holds, as a `ConsistentRead` reports it, and
/// `None` where the item holds no such attribute at all.
async fn held_bytes(router: &MultiServiceRouter, key: &str) -> Option<usize> {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key_of(key), "ConsistentRead": true}),
    )
    .await;
    read["Item"]["held"]["S"].as_str().map(str::len)
}

/// One way of crossing the ceiling, stated as an `UpdateExpression`: the case, the bytes the
/// item is seeded with first, and the `(expression, substitutions)` the change states.
struct Crossing {
    case: &'static str,
    /// A short tag the row's key values are built from, so each row addresses its own item.
    slot: &'static str,
    seeded: usize,
    expression: &'static str,
    values: fn() -> Value,
    /// Whether the update path charges THIS request a ceiling of its own — the part of the
    /// item its clauses leave alone below the floor that part is charged at, or a clause frame
    /// beside the one assignment every such write carries — which is what decides whether a
    /// transaction carrying it is refused or cancelled.
    charged_its_own_ceiling: bool,
}

/// The ways an `UpdateExpression` can leave the item past the ceiling. They differ in WHAT
/// crosses it: one value on its own, a value inside the ceiling over bytes the item already
/// holds, two clauses that only cross together, and a value the update computes from two
/// operands neither of which crosses it — and in whether the key addressed holds an item at
/// all. Each row also differs in whether the update path charges it a ceiling of its OWN,
/// which is the class boundary under test.
fn crossings() -> Vec<Crossing> {
    vec![
        Crossing {
            case: "one value past the ceiling on its own",
            slot: "c1",
            seeded: 0,
            expression: "SET big = :v",
            values: || json!({":v": {"S": "x".repeat(MAX_ITEM_BYTES)}}),
            charged_its_own_ceiling: false,
        },
        Crossing {
            case: "a value inside the ceiling over what the item already holds",
            slot: "c2",
            seeded: 300 * 1024,
            expression: "SET big = :v",
            values: || json!({":v": {"S": "x".repeat(150 * 1024)}}),
            charged_its_own_ceiling: false,
        },
        Crossing {
            case: "two clauses that cross the ceiling only together",
            slot: "c3",
            seeded: 0,
            expression: "SET one = :a, two = :b",
            values: || {
                json!({":a": {"S": "x".repeat(210 * 1024)},
                       ":b": {"S": "y".repeat(210 * 1024)}})
            },
            charged_its_own_ceiling: true,
        },
        Crossing {
            case: "a value the update computes past the ceiling",
            slot: "c4",
            seeded: 0,
            expression: "SET joined = list_append(:a, :b)",
            values: || {
                json!({":a": {"L": [{"S": "x".repeat(210 * 1024)}]},
                       ":b": {"L": [{"S": "y".repeat(210 * 1024)}]}})
            },
            charged_its_own_ceiling: true,
        },
        Crossing {
            case: "a value the update computes from one the item already holds",
            slot: "c5",
            seeded: 210 * 1024,
            expression: "SET joined = list_append(:a, :a)",
            values: || json!({":a": {"L": [{"S": "x".repeat(100 * 1024)}]}}),
            charged_its_own_ceiling: true,
        },
    ]
}

/// The request one crossing states to `UpdateItem`.
fn update_of(crossing: &Crossing, key: &str) -> Value {
    json!({"TableName": TABLE, "Key": key_of(key),
           "UpdateExpression": crossing.expression,
           "ExpressionAttributeValues": (crossing.values)()})
}

/// RULE, the single-item half: a change stated on its own that would leave the item past the
/// ceiling is refused as the request it is, and nothing of it is applied.
#[tokio::test]
async fn an_update_expression_past_the_ceiling_is_refused_on_its_own() {
    let router = seeded().await;

    for crossing in crossings() {
        let case = format!("{} through UpdateItem", crossing.case);
        let key = format!("{}-UpdateItem", crossing.slot);
        if crossing.seeded > 0 {
            seed_item(&router, &key, crossing.seeded).await;
        }
        let (status, body) = call(&router, "UpdateItem", update_of(&crossing, &key)).await;
        assert_refused(&case, status, &body, UPDATE_TOO_BIG);

        // Nothing of the refused change landed: the item is exactly what it was seeded
        // with, and a key that held no item holds none.
        let held = held_bytes(&router, &key).await;
        let expected = (crossing.seeded > 0).then_some(crossing.seeded);
        assert_eq!(
            held, expected,
            "case {case:?}: a refused change leaves the stored item where it was, expected \
             {expected:?} byte(s) of filler, got {held:?}"
        );
    }
}

/// RULE, the transactional half: WHOSE failure the crossing is follows WHICH ceiling the
/// update path held the request to. A request it charges a ceiling of its own — the part of the
/// item its clauses leave alone below the floor that part is charged at, a clause frame beside
/// the one assignment every such write carries — was held to a ceiling below the quota that no
/// stored item can lift, so the CALL is refused exactly as the single-item update of it is, and
/// no action is graded; a request it charges nothing of its own was held to the quota's own
/// count, so a crossing is the circumstance the cancellation documents — the item became too
/// large — and the transaction is cancelled with that action named in the reason list. Nothing
/// is applied either way. (`update_item_ceiling_key_portion.rs` and
/// `transactional_update_clause_charge_class.rs` drive the two charges one at a time.)
#[tokio::test]
async fn an_update_past_the_quota_is_refused_or_cancels_by_which_ceiling_bound_the_request() {
    let router = seeded().await;

    for crossing in crossings() {
        let case = format!("{} through TransactWriteItems", crossing.case);
        let key = format!("{}-TransactWriteItems", crossing.slot);
        if crossing.seeded > 0 {
            seed_item(&router, &key, crossing.seeded).await;
        }
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": [{"Update": update_of(&crossing, &key)}]}),
        )
        .await;
        if crossing.charged_its_own_ceiling {
            assert_refused(&case, status, &body, UPDATE_TOO_BIG);
        } else {
            assert_cancelled(
                &case,
                status,
                &body,
                &[VALIDATION_ERROR_CODE],
                &[Some(UPDATE_TOO_BIG)],
            );
        }

        let held = held_bytes(&router, &key).await;
        let expected = (crossing.seeded > 0).then_some(crossing.seeded);
        assert_eq!(
            held, expected,
            "case {case:?}: a refused or cancelled transaction leaves the stored item where \
             it was, expected {expected:?} byte(s) of filler, got {held:?}"
        );
    }
}

/// The two sides of the ceiling a PartiQL `UPDATE` can cross: a parameter past it on its own,
/// and one inside it over bytes the item already holds. Every row addresses an item that is
/// already there, because a PartiQL `UPDATE` states its target in a `WHERE` clause, so a key
/// that holds no item is a predicate nothing satisfies rather than a change to size.
fn partiql_crossings() -> [(&'static str, &'static str, usize, usize); 2] {
    [
        (
            "p1",
            "one parameter past the ceiling on its own",
            16,
            MAX_ITEM_BYTES,
        ),
        (
            "p2",
            "a parameter inside the ceiling over what the item already holds",
            300 * 1024,
            150 * 1024,
        ),
    ]
}

/// The statement one PartiQL crossing states.
fn statement_of(key: &str, bytes: usize) -> Value {
    json!({
        "Statement": format!("UPDATE \"{TABLE}\" SET big = ? WHERE pk = '{key}'"),
        "Parameters": [{"S": "x".repeat(bytes)}],
    })
}

/// RULE: the language a change is stated in makes no difference either — a PartiQL `UPDATE`
/// past the ceiling is refused run on its own and refused inside a transaction.
#[tokio::test]
async fn a_partiql_update_past_the_ceiling_is_refused_whatever_surface_states_it() {
    let router = seeded().await;

    for (slot, case, seeded, bytes) in partiql_crossings() {
        let case = format!("{case} through ExecuteStatement");
        let key = format!("{slot}-ExecuteStatement");
        seed_item(&router, &key, seeded).await;
        let (status, body) = call(&router, "ExecuteStatement", statement_of(&key, bytes)).await;
        assert_refused(&case, status, &body, UPDATE_TOO_BIG);

        let held = held_bytes(&router, &key).await;
        assert_eq!(
            held,
            Some(seeded),
            "case {case:?}: a refused statement leaves the stored item where it was, \
             expected {seeded} byte(s) of filler, got {held:?}"
        );
    }

    for (slot, case, seeded, bytes) in partiql_crossings() {
        let case = format!("{case} through ExecuteTransaction");
        let key = format!("{slot}-ExecuteTransaction");
        seed_item(&router, &key, seeded).await;
        let (status, body) = call(
            &router,
            "ExecuteTransaction",
            json!({"TransactStatements": [statement_of(&key, bytes)]}),
        )
        .await;
        assert_refused(&case, status, &body, UPDATE_TOO_BIG);

        let held = held_bytes(&router, &key).await;
        assert_eq!(
            held,
            Some(seeded),
            "case {case:?}: a refused statement leaves the stored item where it was, \
             expected {seeded} byte(s) of filler, got {held:?}"
        );
    }
}

/// RULE: the reason list is "ordered according to the ordering of the TransactItems request
/// parameter", so WHERE the cancelling action sits decides where its reason sits and nothing
/// else — every blameless action beside it reports `None`, and an action with a defect of its
/// own reports that one.
///
/// The crossing driven here is the one found against the STORED item: each action asks to
/// store a value any item has room for, and the quota is crossed over the bytes the stored
/// item already held.
#[tokio::test]
async fn where_the_oversized_action_sits_is_where_its_reason_sits() {
    let router = seeded().await;

    // Every key the rows update holds an item already near the ceiling, so what crosses it is
    // what that item holds and not what the action asks for.
    for slot in ["alone", "before", "after", "beside"] {
        seed_item(&router, slot, 300 * 1024).await;
    }
    let grows = json!({":v": {"S": "x".repeat(150 * 1024)}});
    let update = |key: &str| {
        json!({"Update": {"TableName": TABLE, "Key": key_of(key),
               "UpdateExpression": "SET big = :v",
               "ExpressionAttributeValues": grows.clone()}})
    };
    let put = |key: &str| json!({"Put": {"TableName": TABLE, "Item": key_of(key)}});
    let failing_check = || {
        json!({"ConditionCheck": {"TableName": TABLE, "Key": key_of("never-written"),
               "ConditionExpression": "attribute_exists(pk)"}})
    };

    // The rows differ in what stands BESIDE the oversized action: nothing, an action that
    // would succeed on either side of it, and an action whose condition does not hold —
    // each action reporting its OWN outcome in request order.
    for (case, actions, codes, messages, written) in [
        (
            "the oversized action alone",
            json!([update("alone")]),
            vec![VALIDATION_ERROR_CODE],
            vec![Some(UPDATE_TOO_BIG)],
            vec![],
        ),
        (
            "the oversized action before one that would succeed",
            json!([update("before"), put("after-the-ceiling")]),
            vec![VALIDATION_ERROR_CODE, NO_REASON_CODE],
            vec![Some(UPDATE_TOO_BIG), None],
            vec!["after-the-ceiling"],
        ),
        (
            "the oversized action after one that would succeed",
            json!([put("before-the-ceiling"), update("after")]),
            vec![NO_REASON_CODE, VALIDATION_ERROR_CODE],
            vec![None, Some(UPDATE_TOO_BIG)],
            vec!["before-the-ceiling"],
        ),
        (
            "the oversized action beside a condition that does not hold",
            json!([failing_check(), update("beside")]),
            vec!["ConditionalCheckFailed", VALIDATION_ERROR_CODE],
            vec![
                Some("The conditional request failed"),
                Some(UPDATE_TOO_BIG),
            ],
            vec![],
        ),
    ] {
        let (status, body) = call(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": actions}),
        )
        .await;
        assert_cancelled(case, status, &body, &codes, &messages);

        // Nothing a cancelled transaction carried was applied, the sibling `Put` included.
        for key in written {
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": TABLE, "Key": key_of(key), "ConsistentRead": true}),
            )
            .await;
            assert!(
                read.get("Item").is_none(),
                "case {case:?}: the cancelled call wrote nothing, got {read}"
            );
        }
    }
}

/// The CLASSES on either side of the ceiling: a change inside it commits, an action whose own
/// condition is not met cancels with ITS reason rather than the size one, and an item a `Put`
/// carried past the ceiling is the request's own defect.
#[tokio::test]
async fn the_classes_either_side_of_the_ceiling_are_kept() {
    let router = seeded().await;
    seed_item(&router, "committed", 1024).await;

    // A change INSIDE the ceiling commits through the transaction and reads back, so the
    // cancellations above are the ceiling and not a blanket rejection of a transactional
    // update.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": {"TableName": TABLE, "Key": key_of("committed"),
               "UpdateExpression": "SET held = :v",
               "ExpressionAttributeValues": {":v": {"S": "h".repeat(2048)}}}}]}),
    )
    .await;
    let held = held_bytes(&router, "committed").await;
    assert_eq!(
        held,
        Some(2048),
        "case \"a change inside the ceiling\": expected the applied change to read back, got \
         {held:?} byte(s) of filler"
    );

    // An action whose own CONDITION is not met reports the condition it failed, so the size
    // reason above is the ceiling's and not what every cancelled action reports.
    let (status, body) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": {"TableName": TABLE, "Key": key_of("committed"),
               "UpdateExpression": "SET held = :v",
               "ConditionExpression": "attribute_not_exists(pk)",
               "ExpressionAttributeValues": {":v": {"S": "h"}}}}]}),
    )
    .await;
    assert_cancelled(
        "an action whose condition is not met",
        status,
        &body,
        &["ConditionalCheckFailed"],
        &[Some("The conditional request failed")],
    );
    let held = held_bytes(&router, "committed").await;
    assert_eq!(
        held,
        Some(2048),
        "case \"an action whose condition is not met\": a cancelled transaction wrote nothing, \
         got {held:?} byte(s) of filler"
    );

    // An item a `Put` CARRIES past the ceiling is too large as submitted — the request states
    // the whole item, so the defect is the request's and no action is graded for it.
    let (status, body) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Put": {"TableName": TABLE, "Item": {
            "pk": {"S": "carried"},
            "big": {"S": "x".repeat(MAX_ITEM_BYTES + 1)},
        }}}]}),
    )
    .await;
    assert_refused(
        "an item the request carried past the ceiling",
        status,
        &body,
        ITEM_TOO_BIG,
    );
}
