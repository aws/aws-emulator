// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN each kind of failure of a transactional write is FOUND, which
//! is what every `CancellationReason` the call answers with follows from.
//!
//! Driven over the wire (`POST /` with `X-Amz-Target: DynamoDB_20120810.<Operation>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, so every assertion
//! is about what a client sees.
//!
//! RULE 1 — an action's `ConditionExpression` "must be satisfied in order for the
//! conditional operation to succeed" (`Update$ConditionExpression`), so it is read against
//! the stored item BEFORE the change the action states is applied to that item: an action
//! whose condition does not hold reports `ConditionalCheckFailed` and nothing of what
//! applying its `UpdateExpression` to the very same item would have failed on. The rows
//! span the runtime defect the update would have earned (an operand the stored value has
//! the wrong type for, a document path the stored item cannot hold, an attribute it does
//! not carry, a key attribute the update moves), and each
//! row is driven twice — once with the condition failing and once with it holding — so what
//! the rows pin is the ORDER and not one leg of it. The documented item-size quota is one of
//! those defects: the item a change would leave is weighed once that change has been APPLIED,
//! so it too is found after the condition — unless the update path charges the request a
//! ceiling of its OWN (the part of the item the change leaves alone below its floor, a further
//! clause), which is a fact of the REQUEST and refuses the call.
//!
//! RULE 2 — a key that the addressed table's KEY SCHEMA does not admit is found while the
//! action is bound to its table, before any stored item has been read, so the transaction
//! is cancelled there: that action reports `ValidationError` with the key-schema sentence
//! and EVERY other action reports the null code with no message, since nothing of it was
//! ever evaluated. The rows span the position of the malformed key, how many actions stand
//! beside it, and what those actions would themselves have reported had they been
//! evaluated — a condition that does not hold, an update that fails at runtime, and an
//! action nothing is wrong with.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row addresses: one string partition key, so a key stating a number — or
/// naming another attribute — is a key this schema does not admit.
const TABLE: &str = "TransactionalOutcomeOrdering";

/// The sentence a condition that was not met earns.
const CONDITION_FAILED: &str = "The conditional request failed";

/// The sentence a change that would leave the item past the documented item-size quota earns,
/// on every surface that applies one.
const UPDATE_TOO_BIG: &str = "Item size to update has exceeded the maximum allowed size";

/// The sentence a key the table's schema does not admit earns, on every surface that binds
/// one.
const KEY_SCHEMA_MISMATCH: &str = "The provided key element does not match the schema";

/// The `CancellationReason$Code` an action nothing was found wrong with reports.
const NONE: &str = "None";

/// An emulator holding the one active table and the one item the rows read against: a
/// string where an update computes, a map, and a status a condition compares.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the outcome-ordering table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {
            "pk": {"S": "counter"},
            "n": {"S": "not-a-number"},
            "s": {"S": "text"},
            "status": {"S": "ON"},
        }}),
    )
    .await;
    router
}

/// The `(code, message)` every action of a cancelled transaction reports, in request order,
/// and the top-level sentence the call carries.
async fn cancellation(
    router: &MultiServiceRouter,
    case: &str,
    body: Value,
) -> (Vec<(String, Option<String>)>, String) {
    let (status, answered) = call(router, "TransactWriteItems", body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "TransactionCanceledException"),
        "case {case:?}: expected the transaction cancelled, got {status} with {answered}"
    );
    let reported: Vec<(String, Option<String>)> = answered["CancellationReasons"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|reason| {
            (
                reason["Code"].as_str().unwrap_or_default().to_string(),
                reason["Message"].as_str().map(str::to_string),
            )
        })
        .collect();
    (reported, message(&answered))
}

/// "Transaction cancelled, please refer cancellation reasons for specific reasons
/// [<code>, …]": the top-level sentence lists the codes the reason list carries, in the
/// same order, which is what makes it name the reason a caller then finds there.
fn cancelled_message(codes: &[&str]) -> String {
    format!(
        "Transaction cancelled, please refer cancellation reasons for specific reasons [{}]",
        codes.join(", ")
    )
}

/// An `Update` of the seeded item, applying `expression` with the values it substitutes,
/// guarded by a condition on the item's stored status: `expected` is what the condition
/// compares against, so passing the stored value makes it hold and anything else makes it
/// fail.
fn guarded_update(expression: &str, values: Value, expected: &str) -> Value {
    let mut update = json!({
        "TableName": TABLE,
        "Key": {"pk": {"S": "counter"}},
        "UpdateExpression": expression,
        "ConditionExpression": "#s = :expected",
        "ExpressionAttributeNames": {"#s": "status"},
    });
    let mut bound: Value = values;
    bound[":expected"] = json!({"S": expected});
    update["ExpressionAttributeValues"] = bound;
    json!({"Update": update})
}

#[tokio::test]
async fn an_actions_condition_is_read_before_the_change_it_states_is_applied() {
    let router = seeded().await;

    // Each row: the case, the update expression, the values it binds and the sentence
    // applying it to the stored item earns. The rows span the runtime defect that
    // application would be found to have.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "arithmetic on a stored string",
            "SET n = n + :one",
            json!({":one": {"N": "1"}}),
            "An operand in the update expression has an incorrect data type",
        ),
        (
            "a document path through a stored scalar",
            "SET s.deeper = :v",
            json!({":v": {"S": "x"}}),
            "The document path provided in the update expression is invalid for update",
        ),
        (
            "arithmetic on an attribute the item does not carry",
            "SET fresh = absent + :one",
            json!({":one": {"N": "1"}}),
            "The provided expression refers to an attribute that does not exist in the item",
        ),
        (
            "an update that moves the partition key",
            "SET pk = :moved",
            json!({":moved": {"S": "elsewhere"}}),
            "One or more parameter values were invalid: Cannot update attribute pk. This \
             attribute is part of the key",
        ),
    ];

    for (case, expression, values, defect) in rows {
        // The condition does NOT hold: the action has already failed, so the reason is the
        // condition's and the runtime defect of the update is never reached — neither in the
        // reason list nor in the bracketed code list of the top-level sentence.
        let (reported, said) = cancellation(
            &router,
            case,
            json!({"TransactItems": [guarded_update(expression, values.clone(), "OFF")]}),
        )
        .await;
        assert_eq!(
            reported,
            vec![(
                "ConditionalCheckFailed".to_string(),
                Some(CONDITION_FAILED.to_string())
            )],
            "case {case:?} with the condition failing: expected only the condition reported, \
             got {reported:?}"
        );
        assert_eq!(
            said,
            cancelled_message(&["ConditionalCheckFailed"]),
            "case {case:?} with the condition failing: expected the condition named, got \
             {said:?}"
        );

        // And the OTHER leg of the same row: once the condition holds, the arithmetic is
        // reached and the action reports its own defect, with the same sentence the
        // single-item write earns for it.
        let (reported, said) = cancellation(
            &router,
            case,
            json!({"TransactItems": [guarded_update(expression, values.clone(), "ON")]}),
        )
        .await;
        assert_eq!(
            reported,
            vec![("ValidationError".to_string(), Some(defect.to_string()))],
            "case {case:?} with the condition holding: expected {defect:?}, got {reported:?}"
        );
        assert_eq!(
            said,
            cancelled_message(&["ValidationError"]),
            "case {case:?} with the condition holding: expected the validation code named, got \
             {said:?}"
        );
    }

    // The documented item-size quota is weighed over the item the change LEAVES, so it too is
    // found only once that change has been applied — after the condition. The seeded item's
    // own bytes are what carry the result over, and the update path charges this request
    // nothing of its own (one clause, and the part of the item it leaves alone is well past its
    // floor), so the crossing is the transaction's own doing: the row whose condition does not
    // hold reports the condition alone, and the row whose condition holds reports the quota.
    let oversize = json!({":v": {"S": "x".repeat(400 * 1024)}});
    for (case, condition, code, said) in [
        (
            "a statement past the item quota with the condition failing",
            "OFF",
            "ConditionalCheckFailed",
            CONDITION_FAILED,
        ),
        (
            "a statement past the item quota with the condition holding",
            "ON",
            "ValidationError",
            UPDATE_TOO_BIG,
        ),
    ] {
        let (reported, stated) = cancellation(
            &router,
            case,
            json!({"TransactItems": [
                guarded_update("SET big = :v", oversize.clone(), condition)
            ]}),
        )
        .await;
        assert_eq!(
            reported,
            vec![(code.to_string(), Some(said.to_string()))],
            "case {case:?}: expected {code:?} with {said:?}, got {reported:?}"
        );
        assert_eq!(
            stated,
            cancelled_message(&[code]),
            "case {case:?}: expected the code named in the sentence, got {stated:?}"
        );
    }

    // And the one leg that is a fact of the REQUEST: a statement whose only untouched part is
    // the key it addresses — a key that holds no item, so that part is below the floor the
    // update path charges it at — is held to a ceiling of the request's own, which no stored
    // item can lift, so the CALL is refused with the single-item sentence and no action is
    // graded at all.
    let (status, answered) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": [{"Update": {
            "TableName": TABLE,
            "Key": {"pk": {"S": "absent"}},
            "UpdateExpression": "SET big = :v",
            "ExpressionAttributeValues": oversize.clone(),
        }}]}),
    )
    .await;
    assert_eq!(
        (
            status,
            error_code(&answered).as_str(),
            message(&answered).as_str()
        ),
        (400, "ValidationException", UPDATE_TOO_BIG),
        "case \"a statement held to the request's own ceiling\": expected the call refused, got \
         {status} with {answered}"
    );
    assert!(
        answered.get("CancellationReasons").is_none(),
        "case \"a statement held to the request's own ceiling\": a refused call grades no \
         action, so it reports no reason list, got {answered}"
    );

    // NOTHING any of those transactions carried was applied: the seeded item is exactly as
    // it was stored.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "counter"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({
            "pk": {"S": "counter"},
            "n": {"S": "not-a-number"},
            "s": {"S": "text"},
            "status": {"S": "ON"},
        }),
        "case after the cancellations: the seeded item is untouched, got {read}"
    );
}

#[tokio::test]
async fn a_key_the_schema_does_not_admit_blanks_every_other_actions_reason() {
    let router = seeded().await;

    // An action whose key the schema DOES admit, and which would report something of its
    // own had it been evaluated: a condition that does not hold, an update that fails at
    // runtime, and one nothing is wrong with.
    let failing_condition = json!({"ConditionCheck": {
        "TableName": TABLE,
        "Key": {"pk": {"S": "absent"}},
        "ConditionExpression": "attribute_exists(pk)",
    }});
    let failing_update = json!({"Update": {
        "TableName": TABLE,
        "Key": {"pk": {"S": "counter"}},
        "UpdateExpression": "SET n = n + :one",
        "ExpressionAttributeValues": {":one": {"N": "1"}},
    }});
    let sound = json!({"Put": {"TableName": TABLE, "Item": {"pk": {"S": "fresh"}}}});
    // The malformed keys: a value of a type the schema does not declare for the key
    // attribute, and a map naming an attribute the schema declares no key for.
    let wrong_type = json!({"Delete": {"TableName": TABLE, "Key": {"pk": {"N": "1"}}}});
    let wrong_attribute = json!({"Delete": {"TableName": TABLE, "Key": {"other": {"S": "x"}}}});

    // Each row: the case, the actions, and the position of the one whose key the schema
    // does not admit. The rows span that position, the number of actions beside it, what
    // those actions would have reported, and how the key differs from the schema.
    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "a wrong-typed key after a condition that does not hold",
            json!([failing_condition.clone(), wrong_type.clone()]),
            1,
        ),
        (
            "a wrong-typed key before a condition that does not hold",
            json!([wrong_type.clone(), failing_condition.clone()]),
            0,
        ),
        (
            "a wrong-typed key after an update that fails at runtime",
            json!([failing_update.clone(), wrong_type.clone()]),
            1,
        ),
        (
            "a key naming an attribute the schema declares no key for, in the middle",
            json!([
                failing_condition.clone(),
                wrong_attribute.clone(),
                failing_update.clone()
            ]),
            1,
        ),
        (
            "a wrong-typed key beside an action nothing is wrong with",
            json!([sound.clone(), wrong_type.clone(), failing_condition.clone()]),
            1,
        ),
        (
            "the malformed key alone",
            json!([wrong_type.clone()]),
            0,
        ),
    ];

    for (case, actions, at) in rows {
        let elements = actions.as_array().map(Vec::len).unwrap_or_default();
        let (reported, said) = cancellation(
            &router,
            case,
            json!({"TransactItems": actions.clone()}),
        )
        .await;
        let expected: Vec<(String, Option<String>)> = (0..elements)
            .map(|element| {
                if element == at {
                    (
                        "ValidationError".to_string(),
                        Some(KEY_SCHEMA_MISMATCH.to_string()),
                    )
                } else {
                    (NONE.to_string(), None)
                }
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reasons {expected:?}, got {reported:?}"
        );
        let codes: Vec<&str> = (0..elements)
            .map(|element| {
                if element == at {
                    "ValidationError"
                } else {
                    NONE
                }
            })
            .collect();
        assert_eq!(
            said,
            cancelled_message(&codes),
            "case {case:?}: expected the message to list {codes:?}, got {said:?}"
        );
    }

    // The CONTROL: the same actions WITHOUT the malformed key each report their own
    // outcome, so what the rows above pin is the key-schema defect blanking them and not
    // the actions being silent.
    let (reported, _) = cancellation(
        &router,
        "the same actions with every key the schema admits",
        json!({"TransactItems": [failing_condition, failing_update]}),
    )
    .await;
    assert_eq!(
        reported,
        vec![
            (
                "ConditionalCheckFailed".to_string(),
                Some(CONDITION_FAILED.to_string())
            ),
            (
                "ValidationError".to_string(),
                Some("An operand in the update expression has an incorrect data type".to_string())
            ),
        ],
        "case the same actions with every key the schema admits: expected each action's own \
         outcome, got {reported:?}"
    );

    // Nothing any of those transactions carried was applied: the `Put` that stood beside a
    // malformed key left no item behind.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "fresh"}}, "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read.get("Item"),
        None,
        "case after the cancellations: the sound action was part of a cancelled transaction, \
         got {read}"
    );
}
