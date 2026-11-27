// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT A DATA-PLANE CALL ANSWERS when one of the actions, statements
//! or entries it carries cannot be served, and for what the two layers around them read and
//! report.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, so every assertion
//! is about what a client sees — and, where a write is involved, about what a LATER read
//! reports.
//!
//! RULE 1 — a failure a transactional action is found to have by reading it against the
//! STORED item is that ACTION's: the transaction is cancelled, that action's
//! `CancellationReason` carries `ValidationError` with the sentence the same defect earns on
//! the single-item path, and every other action reports its own outcome in request order
//! (`None`, or `ConditionalCheckFailed` for a condition that was not met). The rows span the
//! defect, the position of the failing action and what the actions beside it are.
//!
//! RULE 2 — the VALIDATION class a refusal of a request's own contents carries names the
//! shape the framework that decides it owns, fully qualified, whichever layer decided it.
//! The rows span the layer: a handler's own refusal and a modelled constraint the boundary
//! walk found.
//!
//! RULE 3 — a transactional write costs twice what the same single-item write costs ON THE
//! TABLE, and the secondary-index entries the same write projects are charged once. The rows
//! span the index kind the write fans out to and the action that wrote.
//!
//! RULE 4 — a batch `SELECT` addresses ONE item, so its `WHERE` clause states equality
//! conditions and nothing else; a term that is no equality is reported in that statement's
//! own slot. The rows span the term (a boolean function, a comparison that is not `=`, a
//! range, a disjunction, no clause at all) against the clauses that DO address one item.
//!
//! RULE 5 — a batched read states each of its asks in ONE generation, read over the whole
//! `RequestItems` map: mixing the legacy projection with `ProjectionExpression`, in one
//! entry or across two, is refused, and so is a substitution table the request has no
//! expression for. The rows span which entry states which generation and which entry
//! carries the substitution table.
//!
//! RULE 6 — TEXT that spells a boolean, standing where a boolean is MODELLED, is read as the
//! boolean it spells; text that spells no boolean, and text standing where anything else is
//! modelled, is left exactly as it was sent. The rows span the member, the spelling and the
//! modelled type the text stands at.
//!
//! RULE 7 — a PartiQL write states its source either way round — after the verb
//! (`UPDATE source SET … WHERE …`) or first, in a `FROM` clause
//! (`FROM source WHERE … SET …`) — and runs as the same write; a clause outside the order
//! its form states is a token after a complete statement. The rows span the order, the
//! action clauses and the predicate.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, error_type, message, ok};

/// The table the item rows address: one string partition key.
const ITEMS: &str = "ActionOutcomes";

/// The table the capacity rows address: a composite key with one local and one global
/// secondary index, so a write to it fans out to both kinds.
const INDEXED: &str = "ActionOutcomeIndexes";

/// The local secondary index of [`INDEXED`].
const LOCAL_INDEX: &str = "by-local";

/// The global secondary index of [`INDEXED`].
const GLOBAL_INDEX: &str = "by-global";

/// The shape id the wire carries for the validation class, which is the framework's own and
/// not this service's model: a client that classifies an error by the body's `__type` matches
/// the WHOLE id.
const VALIDATION_SHAPE_ID: &str = "com.amazon.coral.validate#ValidationException";

/// The sentence an `UpdateExpression` whose operand the stored value has the wrong type for
/// earns, on every surface that applies one.
const OPERAND_TYPE: &str = "An operand in the update expression has an incorrect data type";

/// The sentence a document path the stored item cannot hold earns.
const DOCUMENT_PATH: &str = "The document path provided in the update expression is invalid \
                             for update";

/// The sentence a change past the item-update ceiling earns.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The sentence an expression that computes on an attribute the stored item does not carry
/// earns.
const MISSING_ATTRIBUTE: &str = "The provided expression refers to an attribute that does not \
                                 exist in the item";

/// The sentence a condition that was not met earns.
const CONDITION_FAILED: &str = "The conditional request failed";

/// Create a table from `(attribute, key type, attribute type)` rows and poll until it is
/// `ACTIVE`, which is the waiter a client runs after a create.
async fn table(router: &MultiServiceRouter, name: &str, keys: &[(&str, &str, &str)]) {
    let key_schema: Vec<Value> = keys
        .iter()
        .map(|(attribute, role, _)| json!({"AttributeName": attribute, "KeyType": role}))
        .collect();
    let definitions: Vec<Value> = keys
        .iter()
        .map(|(attribute, _, kind)| json!({"AttributeName": attribute, "AttributeType": kind}))
        .collect();
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": key_schema,
            "AttributeDefinitions": definitions,
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
}

/// Create [`ITEMS`] and seed the one item every transactional row reads against: a number, a
/// string and a map, so an action can be given an operand of the wrong type, a document path
/// the item cannot hold and a condition that does not hold.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    table(&router, ITEMS, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": ITEMS, "Item": {
            "pk": {"S": "one"},
            "n": {"N": "5"},
            "s": {"S": "text"},
            "m": {"M": {"kept": {"S": "y"}}},
        }}),
    )
    .await;
    router
}

/// The `CancellationReasons` of a cancelled transaction, as `(code, message)` per action in
/// request order — the list a client attributes a transaction's failure by.
async fn cancellation(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> Vec<(String, Option<String>)> {
    let (status, answered) = call(router, "TransactWriteItems", body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "TransactionCanceledException"),
        "case {case:?}: expected the transaction cancelled, got {status} with {answered}"
    );
    let reasons = answered["CancellationReasons"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let reported: Vec<(String, Option<String>)> = reasons
        .iter()
        .map(|reason| {
            (
                reason["Code"].as_str().unwrap_or_default().to_string(),
                reason["Message"].as_str().map(str::to_string),
            )
        })
        .collect();
    // "Transaction cancelled, please refer cancellation reasons for specific reasons
    // [<code>, …]": the top-level sentence lists the same codes in the same order, which is
    // what makes it name the reason the caller then finds in the list.
    let codes: Vec<String> = reported.iter().map(|(code, _)| code.clone()).collect();
    let expected = format!(
        "Transaction cancelled, please refer cancellation reasons for specific reasons [{}]",
        codes.join(", ")
    );
    assert_eq!(
        message(&answered),
        expected,
        "case {case:?}: expected the message to list {codes:?}, got {answered}"
    );
    reported
}

/// A `Put` action nothing is wrong with, writing its own item.
fn put(key: &str) -> Value {
    json!({"Put": {"TableName": ITEMS, "Item": {"pk": {"S": key}}}})
}

/// An `Update` action applying `expression` to the seeded item, with the values it
/// substitutes.
fn update(expression: &str, values: Value) -> Value {
    json!({"Update": {
        "TableName": ITEMS,
        "Key": {"pk": {"S": "one"}},
        "UpdateExpression": expression,
        "ExpressionAttributeValues": values,
    }})
}

/// A `ConditionCheck` action whose condition does not hold, addressing an item of its OWN:
/// "transaction requests cannot include multiple operations on one item", so a check that
/// stands beside an action on the seeded item has to address a different key — and the key
/// it addresses holds no item, which is what makes `attribute_exists` fail on it.
fn failing_check() -> Value {
    json!({"ConditionCheck": {
        "TableName": ITEMS,
        "Key": {"pk": {"S": "checked"}},
        "ConditionExpression": "attribute_exists(pk)",
    }})
}

/// One row of the cancellation table: the case, the actions the transaction carries, and the
/// `(code, message)` every action reports in request order.
type CancellationRow = (&'static str, Value, Vec<(&'static str, Option<&'static str>)>);

// ===========================================================================
// RULE 1. A transactional action's own failure is that action's reason
// ===========================================================================

#[tokio::test]
async fn an_action_read_against_the_stored_item_reports_its_own_cancellation_reason() {
    let router = seeded().await;

    // Each row: the case, the actions, and the reason every action reports in request order.
    // They span the defect the failing action earns (an operand the stored value has the
    // wrong type for, a document path the stored item cannot hold, an attribute it does not
    // carry), the POSITION of that action, and what
    // stands beside it — nothing, an action that succeeds, an action whose condition fails,
    // and a second failing action.
    let rows: Vec<CancellationRow> = vec![
        (
            "a sole action computing on a stored string",
            json!([update("SET s = s + :one", json!({":one": {"N": "1"}}))]),
            vec![("ValidationError", Some(OPERAND_TYPE))],
        ),
        (
            "an action whose path runs through a stored scalar",
            json!([update("SET s.deeper = :v", json!({":v": {"S": "x"}}))]),
            vec![("ValidationError", Some(DOCUMENT_PATH))],
        ),
        (
            "an action computing on an attribute the item does not carry",
            json!([update("SET fresh = absent + :one", json!({":one": {"N": "1"}}))]),
            vec![("ValidationError", Some(MISSING_ATTRIBUTE))],
        ),
        (
            "an action that succeeds before the failing one",
            json!([
                put("first"),
                update("SET s = s + :one", json!({":one": {"N": "1"}}))
            ]),
            vec![("None", None), ("ValidationError", Some(OPERAND_TYPE))],
        ),
        (
            "an operand-type failure interleaved with a failed condition",
            json!([
                put("second"),
                update("SET s = s + :one", json!({":one": {"N": "1"}})),
                failing_check()
            ]),
            vec![
                ("None", None),
                ("ValidationError", Some(OPERAND_TYPE)),
                ("ConditionalCheckFailed", Some(CONDITION_FAILED)),
            ],
        ),
        (
            "a failed condition before the failing action",
            json!([
                failing_check(),
                update("SET s.deeper = :v", json!({":v": {"S": "x"}}))
            ]),
            vec![
                ("ConditionalCheckFailed", Some(CONDITION_FAILED)),
                ("ValidationError", Some(DOCUMENT_PATH)),
            ],
        ),
        (
            "two failing actions",
            json!([
                update("SET s = s + :one", json!({":one": {"N": "1"}})),
                json!({"Update": {
                    "TableName": ITEMS,
                    "Key": {"pk": {"S": "other"}},
                    "UpdateExpression": "SET deeper.still = :v",
                    "ExpressionAttributeValues": {":v": {"S": "x"}},
                }})
            ]),
            vec![
                ("ValidationError", Some(OPERAND_TYPE)),
                ("ValidationError", Some(DOCUMENT_PATH)),
            ],
        ),
    ];

    for (case, actions, expected) in rows {
        let reported = cancellation(
            &router,
            case,
            json!({"TransactItems": actions.clone()}),
        )
        .await;
        let expected: Vec<(String, Option<String>)> = expected
            .iter()
            .map(|(code, stated)| ((*code).to_string(), stated.map(str::to_string)))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reasons {expected:?}, got {reported:?}"
        );
    }

    // NOTHING any cancelled transaction carried was applied: the seeded item is as it was,
    // and no action that succeeded beside a failing one left its item behind.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["s"],
        json!({"S": "text"}),
        "case after the cancellations: the seeded item is untouched, got {read}"
    );
    for key in ["first", "second"] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": key}}, "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            read.get("Item"),
            None,
            "case after the cancellations: {key:?} was part of a cancelled transaction, got \
             {read}"
        );
    }

    // The CONTROL: the same actions with nothing wrong with them COMMIT, so the rows above
    // are the per-action failure being reported rather than the transaction being refused.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [
            put("committed"),
            update("SET n = n + :one", json!({":one": {"N": "1"}})),
        ]}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["n"],
        json!({"N": "6"}),
        "case the committing control: the applied update reads back, got {read}"
    );
}

/// RULE 1 at the item-size CEILING: WHOSE failure a crossing of the documented 400 KB quota
/// is follows WHICH ceiling the update path held the request to. It weighs an update over the
/// bytes the item it leaves stores PLUS what the request's own shape costs — the part of the
/// item its clauses leave alone charged at a floor, the frame each further clause states — so a
/// request charged anything of its own was held to a ceiling under the quota that no stored
/// item can lift: crossing it is a fact of the REQUEST, the CALL is refused with the sentence
/// the update path words that quota with and no action is graded. A request charged nothing of
/// its own was held to the quota's own count, so a crossing there is "an item size becomes too
/// large (larger than 400 KB) [...] because of changes made by the transaction", one of the
/// circumstances `TransactionCanceledException` documents, so the transaction is CANCELLED and
/// the reason list names that action with the same sentence, every blameless action beside it
/// reporting `None`.
///
/// The rows differ along what the rule generalises over: WHAT the request is charged for (two
/// clauses that only cross together, a clause whose operand is a call), WHERE the crossing
/// action sits (alone, before an action that would succeed, after one, beside an action whose
/// condition does not hold) — which is what the reason list reports at each action's own index
/// once the crossing is the transaction's own doing — and, for the cancelling rows, a small
/// value over an item already near the quota.
///
/// The other side of the rule is driven beside them: an item a `Put` CARRIES past the ceiling
/// is too large as SUBMITTED — a fact of the REQUEST, settled before any stored item is read —
/// so it refuses the CALL with the sentence for an item that was carried and grades no action.
#[tokio::test]
async fn an_update_past_the_item_quota_answers_by_which_ceiling_bound_the_request() {
    let router = seeded().await;

    // Past the documented 400 KB item ceiling in two clauses that only cross it together, so
    // the update path charges this request the second clause's own frame beside the item.
    let half = "y".repeat(210 * 1024);
    let two_clauses = || {
        update(
            "SET one = :a, two = :b",
            json!({":a": {"S": half.clone()}, ":b": {"S": half.clone()}}),
        )
    };
    // Each row: the case and the actions the transaction carries. Every one of them is charged
    // a ceiling of the request's own, so every one REFUSES the call, whatever stands beside it.
    let charged_its_own_ceiling: Vec<(&str, Value)> = vec![
        ("the oversized action alone", json!([two_clauses()])),
        (
            "the oversized action before one that would succeed",
            json!([two_clauses(), put("after-the-ceiling")]),
        ),
        (
            "the oversized action after one that would succeed",
            json!([put("before-the-ceiling"), two_clauses()]),
        ),
        (
            "the oversized action beside a condition that does not hold",
            json!([failing_check(), two_clauses()]),
        ),
        (
            // The clause's operand is a CALL, whose frame the request is charged whatever the
            // item it leaves holds: the value the update COMPUTES crosses the ceiling, and
            // both operands are the request's own.
            "a value the update computes past the ceiling from what it carried",
            json!([update(
                "SET joined = list_append(:a, :b)",
                json!({":a": {"L": [{"S": half.clone()}]}, ":b": {"L": [{"S": half.clone()}]}})
            )]),
        ),
    ];

    for (case, actions) in charged_its_own_ceiling {
        let (status, answered) =
            call(&router, "TransactWriteItems", json!({"TransactItems": actions})).await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message(&answered).as_str()
            ),
            (400, "ValidationException", UPDATE_TOO_BIG),
            "case {case:?}: expected the call refused for the ceiling the request was held to, \
             got {status} with {answered}"
        );
        assert!(
            answered.get("CancellationReasons").is_none(),
            "case {case:?}: a refused call grades no action, so it reports no reason list, got \
             {answered}"
        );
    }

    // The other half of the rule: an item already near the ceiling, and a change asking for
    // bytes any item has room for — the item it leaves is past the quota because of the bytes
    // the stored item already held, so the transaction is cancelled and every action beside it
    // reports its own outcome in request order.
    ok(
        &router,
        "PutItem",
        json!({"TableName": ITEMS, "Item": {
            "pk": {"S": "near-the-ceiling"},
            "held": {"S": "h".repeat(390 * 1024)},
        }}),
    )
    .await;
    let grows = json!({"Update": {
        "TableName": ITEMS,
        "Key": {"pk": {"S": "near-the-ceiling"}},
        "UpdateExpression": "SET big = :v",
        "ExpressionAttributeValues": {":v": {"S": "g".repeat(20 * 1024)}},
    }});
    let over_the_stored_item: Vec<CancellationRow> = vec![
        (
            "a change the stored item's own bytes push past the ceiling",
            json!([grows.clone()]),
            vec![("ValidationError", Some(UPDATE_TOO_BIG))],
        ),
        (
            "that change after an action that would succeed",
            json!([put("before-the-stored-ceiling"), grows.clone()]),
            vec![("None", None), ("ValidationError", Some(UPDATE_TOO_BIG))],
        ),
        (
            "that change beside a condition that does not hold",
            json!([grows.clone(), failing_check()]),
            vec![
                ("ValidationError", Some(UPDATE_TOO_BIG)),
                ("ConditionalCheckFailed", Some(CONDITION_FAILED)),
            ],
        ),
    ];

    for (case, actions, expected) in over_the_stored_item {
        let reported = cancellation(&router, case, json!({"TransactItems": actions})).await;
        let expected: Vec<(String, Option<String>)> = expected
            .iter()
            .map(|(code, stated)| ((*code).to_string(), stated.map(str::to_string)))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reasons {expected:?}, got {reported:?}"
        );
    }

    // The BOUNDARY: an item a `Put` carries past the ceiling is the request's own defect, so
    // the call is refused with the sentence for an item that was CARRIED and no reason list.
    let (status, answered) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Put": {"TableName": ITEMS, "Item": {
            "pk": {"S": "carried-past-the-ceiling"},
            "big": {"S": "x".repeat(400 * 1024)},
        }}}]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str(), message(&answered)),
        (
            400,
            "ValidationException",
            "Item size has exceeded the maximum allowed size".to_string()
        ),
        "case \"an item the request carried past the ceiling\": expected the carried-item \
         sentence, got {status} with {answered}"
    );
    assert!(
        answered.get("CancellationReasons").is_none(),
        "case \"an item the request carried past the ceiling\": a refused call cancels \
         nothing, so it reports no reason list, got {answered}"
    );

    // Nothing any refused or cancelled call carried was applied, and the control commits: an
    // update that stays inside the ceiling is served on the same item.
    for key in [
        "after-the-ceiling",
        "before-the-ceiling",
        "before-the-stored-ceiling",
        "carried-past-the-ceiling",
    ] {
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": key}}, "ConsistentRead": true}),
        )
        .await;
        assert!(
            read.get("Item").is_none(),
            "case {key:?}: the call that did not go through wrote nothing, got {read}"
        );
    }
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [update(
            "SET inside = :v",
            json!({":v": {"S": "y".repeat(1024)}})
        )]}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["inside"]["S"].as_str().map(str::len),
        Some(1024),
        "case the committing control: an update inside the ceiling is applied, got {read}"
    );
}

// ===========================================================================
// RULE 2. The validation class names the whole shape id
// ===========================================================================

#[tokio::test]
async fn every_validation_refusal_names_the_qualified_shape_id() {
    let router = seeded().await;

    // Each row: the case and the request, spanning the LAYER that decides the refusal — a
    // handler's own rule, a bound the model declares (found by the boundary walk), and a
    // datatype defect read off the body before any operation ran — because the class is the
    // same whichever layer decided it.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a handler's own rule: a query stating no key condition",
            "Query",
            json!({"TableName": ITEMS}),
        ),
        (
            "a handler's own quota: a value nested past the supported depth",
            "PutItem",
            json!({"TableName": ITEMS, "Item": {"pk": {"S": "deep"}, "tree": nested(40)}}),
        ),
        (
            "a bound the model declares: a page size below its range",
            "ListTables",
            json!({"Limit": 0}),
        ),
        (
            "a bound the model declares: a start key shorter than a table name may be",
            "ListTables",
            json!({"ExclusiveStartTableName": "ab"}),
        ),
        (
            "a transactional action nested past the supported depth",
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {"TableName": ITEMS,
                   "Item": {"pk": {"S": "deep"}, "tree": nested(40)}}}]}),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_type(&answered).as_str()),
            (400, VALIDATION_SHAPE_ID),
            "case {case:?} ({operation}): expected the qualified validation shape id, got \
             {status} with {answered}"
        );
    }

    // The BOUNDARY: an error the MODEL declares keeps the shape id of this service's own
    // namespace, so the rows above are the framework's class being named rather than every
    // error being renamed.
    let (status, answered) = call(
        &router,
        "GetItem",
        json!({"TableName": "NoSuchTableAtAll", "Key": {"pk": {"S": "x"}}}),
    )
    .await;
    assert_eq!(
        (status, error_type(&answered).as_str()),
        (
            400,
            "com.amazonaws.dynamodb.v20120810#ResourceNotFoundException"
        ),
        "case a modelled error: expected this service's own qualified shape id, got {status} \
         with {answered}"
    );
}

/// A value nesting `levels` deep, for the rows about the documented nesting quota.
fn nested(levels: usize) -> Value {
    let mut value = json!({"S": "leaf"});
    for _ in 0..levels {
        value = json!({"L": [value]});
    }
    value
}

// ===========================================================================
// RULE 3. A transactional write doubles the TABLE's units, not the indexes'
// ===========================================================================

#[tokio::test]
async fn a_transactional_write_doubles_the_table_units_and_charges_each_index_once() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": INDEXED,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "lsk", "AttributeType": "S"},
                {"AttributeName": "gpk", "AttributeType": "S"},
            ],
            "LocalSecondaryIndexes": [{
                "IndexName": LOCAL_INDEX,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "lsk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "GlobalSecondaryIndexes": [{
                "IndexName": GLOBAL_INDEX,
                "KeySchema": [{"AttributeName": "gpk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    describe_when_active(&router, "the indexed table", INDEXED).await;

    // Each row: the case, the item the action writes and which indexes that item is
    // projected into. They span the index kind the write fans out to: neither, the local
    // index, the global one, and both.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "an item projected into neither index",
            json!({"pk": {"S": "p"}, "sk": {"S": "a"}}),
            vec![],
        ),
        (
            "an item projected into the local index",
            json!({"pk": {"S": "p"}, "sk": {"S": "b"}, "lsk": {"S": "l"}}),
            vec![LOCAL_INDEX],
        ),
        (
            "an item projected into the global index",
            json!({"pk": {"S": "p"}, "sk": {"S": "c"}, "gpk": {"S": "g"}}),
            vec![GLOBAL_INDEX],
        ),
        (
            "an item projected into both indexes",
            json!({"pk": {"S": "p"}, "sk": {"S": "d"},
                   "lsk": {"S": "l"}, "gpk": {"S": "g"}}),
            vec![LOCAL_INDEX, GLOBAL_INDEX],
        ),
    ];

    for (case, item, indexes) in rows {
        // The same write, once on its own and once inside a transaction: the single-item
        // figures are what the transactional ones are read against, so the factor is
        // measured rather than written down. Each of the two addresses its OWN item, because
        // a write that leaves an index entry exactly as it stood changes nothing in that
        // index and is charged nothing for it.
        let single = ok(
            &router,
            "PutItem",
            json!({"TableName": INDEXED, "Item": addressed(&item, "single"),
                   "ReturnConsumedCapacity": "INDEXES"}),
        )
        .await;
        let transactional = ok(
            &router,
            "TransactWriteItems",
            json!({"TransactItems": [{"Put": {"TableName": INDEXED,
                                              "Item": addressed(&item, "transactional")}}],
                   "ReturnConsumedCapacity": "INDEXES"}),
        )
        .await;
        let single = single["ConsumedCapacity"].clone();
        let reported = transactional["ConsumedCapacity"][0].clone();
        // "Transactional write requests require two write capacity units to perform one
        // write per second for items up to 1 KB": the factor is the cost of the ITEM write,
        // so the table's own figure doubles.
        let table_units = single["Table"]["CapacityUnits"].as_f64().unwrap_or_default();
        assert_eq!(
            reported["Table"]["CapacityUnits"].as_f64(),
            Some(table_units * 2.0),
            "case {case:?}: expected the table's units doubled from {table_units}, got \
             {reported}"
        );
        // The secondary-index entries the same write projects are charged what one write of
        // them costs, so each index figure is the single-item one.
        let mut total = table_units * 2.0;
        for index in &indexes {
            let group = if *index == LOCAL_INDEX {
                "LocalSecondaryIndexes"
            } else {
                "GlobalSecondaryIndexes"
            };
            let units = single[group][*index]["CapacityUnits"]
                .as_f64()
                .unwrap_or_default();
            assert!(
                units > 0.0,
                "case {case:?}: the single-item write must report {index:?}, got {single}"
            );
            assert_eq!(
                reported[group][*index]["CapacityUnits"].as_f64(),
                Some(units),
                "case {case:?}: expected {index:?} charged {units} once, got {reported}"
            );
            total += units;
        }
        // "The total number of capacity units consumed by the operation" is the sum of the
        // table's and every index's, so the aggregate follows from the two rules above.
        assert_eq!(
            reported["CapacityUnits"].as_f64(),
            Some(total),
            "case {case:?}: expected the aggregate {total}, got {reported}"
        );
        assert_eq!(
            reported["WriteCapacityUnits"].as_f64(),
            Some(total),
            "case {case:?}: a write bills the aggregate as write capacity, got {reported}"
        );
    }
}

/// `item` with `which` appended to its sort key, so the two writes of one row address two
/// items of the same partition and each one is a write that lands.
fn addressed(item: &Value, which: &str) -> Value {
    let mut addressed = item.clone();
    let sort = item["sk"]["S"].as_str().unwrap_or_default();
    addressed["sk"] = json!({"S": format!("{sort}-{which}")});
    addressed
}

// ===========================================================================
// RULE 4. A batch SELECT's WHERE clause states equalities only
// ===========================================================================

/// The sentence a batch read that does not address ONE item of the table by its primary key
/// is refused with, in its own slot.
const KEY_EQUALITY: &str = "Select statements within BatchExecuteStatement must specify the \
                            primary key in the where clause.";

#[tokio::test]
async fn a_batch_select_states_equality_conditions_and_nothing_else() {
    let router = emulator();
    table(&router, ITEMS, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": ITEMS, "Item": {"pk": {"S": "one"}, "note": {"S": "kept"},
                                            "rank": {"N": "3"}}}),
    )
    .await;

    // Each row: the case, the statement, and whether the slot carries the refusal. The
    // refused rows span the term that is no equality — a boolean function beside the key
    // equality, a comparison that is not `=`, a range, a membership test, a disjunction, a
    // negation, a comparison of two paths — and the clause that states none at all; the
    // served rows are the clauses that DO address one item, with and without a further
    // equality the stored item is held to.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a boolean function beside the key equality",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND attribute_exists(note)"),
            true,
        ),
        (
            "a boolean function the stored item does not satisfy",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND attribute_exists(gone)"),
            true,
        ),
        (
            "a comparison that is not an equality",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND rank > 1"),
            true,
        ),
        (
            "a range beside the key equality",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND rank BETWEEN 1 AND 5"),
            true,
        ),
        (
            "a membership test beside the key equality",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND note IN ['kept']"),
            true,
        ),
        (
            "a disjunction of two key equalities",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' OR pk = 'two'"),
            true,
        ),
        (
            "a negated key equality",
            format!("SELECT * FROM \"{ITEMS}\" WHERE NOT pk = 'two'"),
            true,
        ),
        (
            "no clause at all",
            format!("SELECT * FROM \"{ITEMS}\""),
            true,
        ),
        (
            "the key equality alone",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one'"),
            false,
        ),
        (
            "the key equality beside a further equality the item satisfies",
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND note = 'kept'"),
            false,
        ),
        (
            "the key equality written the other way round",
            format!("SELECT * FROM \"{ITEMS}\" WHERE 'one' = pk"),
            false,
        ),
    ];

    for (case, statement, refused) in rows {
        let (status, answered) = call(
            &router,
            "BatchExecuteStatement",
            json!({"Statements": [{"Statement": statement.clone()}]}),
        )
        .await;
        // "A HTTP 200 response does not mean that all statements in the
        // BatchExecuteStatement succeeded": a statement's own failure is its slot's.
        assert_eq!(
            status, 200,
            "case {case:?}: a batch answers 200 whatever its slots say, got {status} with \
             {answered}"
        );
        let slot = answered["Responses"][0].clone();
        if refused {
            assert_eq!(
                (
                    slot["Error"]["Code"].as_str(),
                    slot["Error"]["Message"].as_str()
                ),
                (Some("ValidationError"), Some(KEY_EQUALITY)),
                "case {case:?} ({statement}): expected the per-slot refusal, got {slot}"
            );
        } else {
            assert_eq!(
                slot.get("Error"),
                None,
                "case {case:?} ({statement}): expected the statement served, got {slot}"
            );
            assert_eq!(
                slot["Item"]["pk"],
                json!({"S": "one"}),
                "case {case:?} ({statement}): expected the addressed item, got {slot}"
            );
        }
    }

    // The CONTROL: a further equality the stored item does NOT satisfy is applied to the
    // item the key equalities address, so that slot reports no item and no error — which is
    // the answer a clause of equalities earns, as against the refusal above.
    let (status, answered) = call(
        &router,
        "BatchExecuteStatement",
        json!({"Statements": [{"Statement":
            format!("SELECT * FROM \"{ITEMS}\" WHERE pk = 'one' AND note = 'other'")}]}),
    )
    .await;
    assert_eq!(
        (status, answered["Responses"][0].get("Error")),
        (200, None),
        "case an equality the item does not satisfy: expected the statement served, got \
         {answered}"
    );
    assert_eq!(
        answered["Responses"][0].get("Item"),
        None,
        "case an equality the item does not satisfy: expected no item, got {answered}"
    );
}

// ===========================================================================
// RULE 5. A batched read states its projections in one generation
// ===========================================================================

#[tokio::test]
async fn each_batch_read_entry_is_held_to_its_own_projection() {
    let router = emulator();
    let other = "ActionOutcomesOther";
    table(&router, ITEMS, &[("pk", "HASH", "S")]).await;
    table(&router, other, &[("pk", "HASH", "S")]).await;
    for name in [ITEMS, other] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"pk": {"S": "one"}, "note": {"S": "kept"}}}),
        )
        .await;
    }

    // Each row: the case and the two entries' options. Every one of them states each
    // entry's ask exactly once, in ONE generation across the whole request, so every one is
    // served. They span which generation the request uses, which entry carries a
    // substitution table, and whose projection dereferences it.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "the legacy member in both entries",
            json!({"Keys": [{"pk": {"S": "one"}}], "AttributesToGet": ["pk"]}),
            json!({"Keys": [{"pk": {"S": "one"}}], "AttributesToGet": ["pk"]}),
        ),
        (
            "the expression member in both entries",
            json!({"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "pk"}),
            json!({"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "pk"}),
        ),
        (
            "a names table used by its own entry's projection beside a plain entry",
            json!({"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "#n",
                   "ExpressionAttributeNames": {"#n": "note"}}),
            json!({"Keys": [{"pk": {"S": "one"}}]}),
        ),
        (
            "a names table with no projection beside an entry that projects",
            json!({"Keys": [{"pk": {"S": "one"}}],
                   "ExpressionAttributeNames": {"#n": "note"}}),
            json!({"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "pk"}),
        ),
        (
            "both entries projecting through their own names table",
            json!({"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "#n",
                   "ExpressionAttributeNames": {"#n": "note"}}),
            json!({"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "#p",
                   "ExpressionAttributeNames": {"#p": "pk"}}),
        ),
    ];

    for (case, first, second) in rows {
        let served = ok(
            &router,
            "BatchGetItem",
            json!({"RequestItems": {ITEMS: first.clone(), other: second.clone()}}),
        )
        .await;
        assert_eq!(
            served["Responses"][ITEMS].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the first entry's item, got {served}"
        );
        assert_eq!(
            served["Responses"][other].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the second entry's item, got {served}"
        );
        // "A map of tables and their respective keys that were not processed": empty when
        // every key the request addressed was read, which is what lets a drain loop stop.
        assert_eq!(
            served["UnprocessedKeys"],
            json!({}),
            "case {case:?}: expected an empty UnprocessedKeys map, got {served}"
        );
    }

    // The BOUNDARY: the generations are read over the WHOLE request, so a batch that states
    // its projections in both — inside one entry or across two — is refused, and so is one
    // whose names table has no projection anywhere to go into.
    for (case, request, expected) in [
        (
            "one entry stating both generations",
            json!({"RequestItems": {ITEMS: {
                "Keys": [{"pk": {"S": "one"}}],
                "AttributesToGet": ["pk"],
                "ProjectionExpression": "pk"}}}),
            "Can not use both expression and non-expression parameters in the same request: \
             Non-expression parameters: {AttributesToGet} Expression parameters: \
             {ProjectionExpression}",
        ),
        (
            "one entry projecting the legacy way and the other the expression way",
            json!({"RequestItems": {
                ITEMS: {"Keys": [{"pk": {"S": "one"}}], "AttributesToGet": ["pk"]},
                other: {"Keys": [{"pk": {"S": "one"}}], "ProjectionExpression": "pk"}}}),
            "Can not use both expression and non-expression parameters in the same request: \
             Non-expression parameters: {AttributesToGet} Expression parameters: \
             {ProjectionExpression}",
        ),
        (
            "a names table beside the legacy projection of the same entry",
            json!({"RequestItems": {ITEMS: {
                "Keys": [{"pk": {"S": "one"}}],
                "AttributesToGet": ["pk"],
                "ExpressionAttributeNames": {"#n": "note"}}}}),
            "ExpressionAttributeNames can only be specified when using expressions",
        ),
        (
            "a names table in a batch that projects nowhere",
            json!({"RequestItems": {
                ITEMS: {"Keys": [{"pk": {"S": "one"}}],
                        "ExpressionAttributeNames": {"#n": "note"}},
                other: {"Keys": [{"pk": {"S": "one"}}]}}}),
            "ExpressionAttributeNames can only be specified when using expressions",
        ),
    ] {
        let (status, answered) = call(&router, "BatchGetItem", request).await;
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                message(&answered).as_str()
            ),
            (400, "ValidationException", expected),
            "case {case:?}: expected {expected:?}, got {status} with {answered}"
        );
    }
}

// ===========================================================================
// RULE 6. Text that spells a boolean is read as that boolean
// ===========================================================================

#[tokio::test]
async fn text_that_spells_a_boolean_is_read_where_a_boolean_is_modelled() {
    let router = emulator();
    table(&router, ITEMS, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": ITEMS, "Item": {"pk": {"S": "one"}, "flag": {"S": "true"}}}),
    )
    .await;

    // Each row: the case, the operation and the request. Every one of them carries a boolean
    // member as TEXT that spells a boolean, and every one is SERVED — the rows span the
    // member (a read's consistency, a scan's, a query's traversal direction), the spelling
    // and where the member sits (the request's own members and one entry of a batch).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a read asking for a consistent read in text",
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}},
                   "ConsistentRead": "true"}),
        ),
        (
            "a read asking for an eventually consistent one in text",
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}},
                   "ConsistentRead": "false"}),
        ),
        (
            "a scan's consistency in text",
            "Scan",
            json!({"TableName": ITEMS, "ConsistentRead": "true"}),
        ),
        (
            "a query's traversal direction in text",
            "Query",
            json!({"TableName": ITEMS, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}},
                   "ScanIndexForward": "false"}),
        ),
        (
            "one entry of a batched read asking for consistency in text",
            "BatchGetItem",
            json!({"RequestItems": {ITEMS: {"Keys": [{"pk": {"S": "one"}}],
                                            "ConsistentRead": "true"}}}),
        ),
    ];

    for (case, operation, request) in rows {
        let served = ok(&router, operation, request).await;
        assert!(
            served.is_object(),
            "case {case:?} ({operation}): expected the request served, got {served}"
        );
    }

    // The item the rows above read is UNCHANGED by the reading, and the attribute whose
    // own text spells a boolean is still the STRING it was stored as: the conversion is
    // about where a boolean is MODELLED and nothing else.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}}, "ConsistentRead": "true"}),
    )
    .await;
    assert_eq!(
        read["Item"]["flag"],
        json!({"S": "true"}),
        "case an attribute whose text spells a boolean: expected the stored string, got {read}"
    );

    // And text that spells NO boolean where a boolean is modelled is still a value the
    // member cannot be read from, as is a boolean's text where anything else is modelled.
    for (case, operation, request, expected) in [
        (
            "text that spells no boolean",
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}},
                   "ConsistentRead": "maybe"}),
            "STRING_VALUE cannot be converted to Boolean",
        ),
        (
            "a number where a boolean is modelled",
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}}, "ConsistentRead": 1}),
            "NUMBER_VALUE cannot be converted to Boolean",
        ),
        (
            "a boolean where a table name is modelled",
            "GetItem",
            json!({"TableName": true, "Key": {"pk": {"S": "one"}}}),
            "TRUE_VALUE cannot be converted to String",
        ),
    ] {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (
                status,
                error_type(&answered).as_str(),
                answered["Message"].as_str()
            ),
            (
                400,
                "com.amazon.coral.service#SerializationException",
                Some(expected)
            ),
            "case {case:?}: expected the value refused as unreadable, got {status} with \
             {answered}"
        );
    }
}

// ===========================================================================
// RULE 7. A PartiQL write states its source either way round
// ===========================================================================

#[tokio::test]
async fn a_partiql_write_runs_in_either_clause_order() {
    let router = emulator();
    table(&router, ITEMS, &[("pk", "HASH", "S")]).await;

    // Each row: the case, the statement and the value `status` holds afterwards — `None`
    // where the statement is refused and the stored item is left alone. The served rows span
    // the order the clauses come in (the verb naming the target, the source stated first in
    // a `FROM` clause) and the action clauses (one assignment, two, an assignment beside a
    // removal); the refused rows span the clause stated outside its form's order and the
    // statement that states no action at all.
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        (
            "the verb naming the target",
            format!("UPDATE \"{ITEMS}\" SET status = 'confirmed' WHERE pk = 'one'"),
            Some("confirmed"),
        ),
        (
            "the source stated first",
            format!("FROM \"{ITEMS}\" WHERE pk = 'one' SET status = 'confirmed'"),
            Some("confirmed"),
        ),
        (
            "the source stated first, with two assignments",
            format!(
                "FROM \"{ITEMS}\" WHERE pk = 'one' SET status = 'confirmed' SET seen = 1"
            ),
            Some("confirmed"),
        ),
        (
            "the source stated first, with an assignment and a removal",
            format!("FROM \"{ITEMS}\" WHERE pk = 'one' SET status = 'confirmed' REMOVE note"),
            Some("confirmed"),
        ),
        (
            "the source stated first, with the predicate parameterised",
            format!("FROM \"{ITEMS}\" WHERE pk = ? SET status = 'confirmed'"),
            Some("confirmed"),
        ),
        (
            "the source stated first with the predicate after the actions",
            format!("FROM \"{ITEMS}\" SET status = 'moved' WHERE pk = 'one'"),
            None,
        ),
        (
            "the verb naming the target with the predicate before the actions",
            format!("UPDATE \"{ITEMS}\" WHERE pk = 'one' SET status = 'moved'"),
            None,
        ),
        (
            "the source stated first and no action at all",
            format!("FROM \"{ITEMS}\" WHERE pk = 'one'"),
            None,
        ),
    ];

    for (case, statement, applied) in rows {
        ok(
            &router,
            "PutItem",
            json!({"TableName": ITEMS, "Item": {"pk": {"S": "one"},
                                                "status": {"S": "active"},
                                                "note": {"S": "kept"}}}),
        )
        .await;
        let mut request = json!({"Statement": statement.clone()});
        if statement.contains('?') {
            request["Parameters"] = json!([{"S": "one"}]);
        }
        let (status, answered) = call(&router, "ExecuteStatement", request).await;
        let expected_status = if applied.is_some() { 200 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {case:?} ({statement}): expected HTTP {expected_status}, got {status} with \
             {answered}"
        );
        // A write states its own answer either way: the statement either applied, and the
        // read below reports what it left, or it was refused and the item is as it was.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": ITEMS, "Key": {"pk": {"S": "one"}}, "ConsistentRead": true}),
        )
        .await;
        let expected = json!({"S": applied.unwrap_or("active")});
        assert_eq!(
            read["Item"]["status"], expected,
            "case {case:?} ({statement}): expected status {expected}, got {read}"
        );
    }

    // A predicate that pins no key addresses no item on either order, so both forms earn
    // the same rule — which is what makes the order a matter of the grammar alone.
    for (case, statement) in [
        (
            "the verb naming the target",
            format!("UPDATE \"{ITEMS}\" SET status = 'moved' WHERE note = 'kept'"),
        ),
        (
            "the source stated first",
            format!("FROM \"{ITEMS}\" WHERE note = 'kept' SET status = 'moved'"),
        ),
    ] {
        let (status, answered) = call(
            &router,
            "ExecuteStatement",
            json!({"Statement": statement.clone()}),
        )
        .await;
        assert_eq!(
            (status, message(&answered).as_str()),
            (
                400,
                "Where clause does not contain a mandatory equality on all key attributes"
            ),
            "case {case:?} ({statement}): expected the key-equality rule, got {status} with \
             {answered}"
        );
    }
}
