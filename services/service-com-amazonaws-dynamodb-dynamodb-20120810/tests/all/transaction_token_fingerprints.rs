// SPDX-License-Identifier: CC0-1.0

//! What a `ClientRequestToken` STANDS FOR: the whole request it was first used with.
//!
//! The rule: a token inside its idempotency window stands for the request that minted
//! it, so a retry carrying the same token and the same request is answered from what
//! that call did and applies nothing, while a retry that changes ANY member of the
//! request — the actions it carries or the detail it asks the response to report — is
//! refused 400 `IdempotentParameterMismatchException` ("DynamoDB rejected the request
//! because you retried a request with a different payload but with an idempotent token
//! that was already used") with the fixed sentence below, and applies nothing either.
//!
//! An omitted `Return*` member asks for the level the service defaults it to ("If set to
//! NONE (the default), no statistics are returned" —
//! `TransactWriteItems$ReturnItemCollectionMetrics`), so omitting it and spelling that
//! level are the same request.
//!
//! Driven as a TABLE spanning what the rule generalises over: which member the retry
//! changed (`ReturnConsumedCapacity` at each of its three levels,
//! `ReturnItemCollectionMetrics` at each of its two, and the actions themselves), in
//! which direction it changed, and which operation a token makes idempotent
//! (`TransactWriteItems` and `ExecuteTransaction`). The enum levels come from the model:
//! `smithy_query(query="shape", shape="com.amazonaws.dynamodb#ReturnConsumedCapacity")`
//! gives INDEXES | TOTAL | NONE and `ReturnItemCollectionMetrics` gives SIZE | NONE.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table the rows drive, keyed on one string partition key.
const TABLE: &str = "TokenFingerprints";

/// AWS's fixed sentence for a token re-used with a different request: no part of it is
/// taken from the request.
const MISMATCH: &str = "Specified idempotent token was used with different request parameters \
                        within the idempotency window";

/// The counter item every transaction below increments.
fn key() -> Value {
    json!({"pk": {"S": "ctr"}})
}

/// A table holding the counter at zero.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {"pk": {"S": "ctr"}, "n": {"N": "0"}}}),
    )
    .await;
    router
}

/// The counter's value right now.
async fn counter(router: &MultiServiceRouter) -> String {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
    )
    .await;
    read["Item"]["n"]["N"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A `TransactWriteItems` body: the ADD that increments the counter under `token`, with
/// whichever reporting members the row states.
fn transaction(token: &str, reporting: &Value, by: &str) -> Value {
    let mut request = json!({
        "ClientRequestToken": token,
        "TransactItems": [{"Update": {
            "TableName": TABLE,
            "Key": key(),
            "UpdateExpression": "ADD n :by",
            "ExpressionAttributeValues": {":by": {"N": by}},
        }}],
    });
    for (member, value) in reporting.as_object().expect("an object of members") {
        request[member] = value.clone();
    }
    request
}

/// Require the refusal AWS states for a token re-used with a different request.
async fn expect_mismatch(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) {
    let (status, answered) = call(router, operation, body.clone()).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "IdempotentParameterMismatchException"),
        "case {case:?}: expected 400 IdempotentParameterMismatchException, got {status} with \
         {answered}"
    );
    assert_eq!(
        message(&answered),
        MISMATCH,
        "case {case:?}: expected AWS's fixed mismatch sentence, got {}",
        message(&answered)
    );
}

#[tokio::test]
async fn a_retry_that_changes_a_reporting_member_is_the_documented_mismatch() {
    let router = seeded().await;

    // Each row: the case, the reporting members the FIRST call stated, and the members
    // the retry stated instead. They span both reporting members, every level each one
    // models, adding a member that was absent, dropping one that was present, and
    // changing a level for another.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "TOTAL retried as NONE",
            json!({"ReturnConsumedCapacity": "TOTAL"}),
            json!({"ReturnConsumedCapacity": "NONE"}),
        ),
        (
            "TOTAL retried as INDEXES",
            json!({"ReturnConsumedCapacity": "TOTAL"}),
            json!({"ReturnConsumedCapacity": "INDEXES"}),
        ),
        (
            "nothing asked for, retried asking for TOTAL",
            json!({}),
            json!({"ReturnConsumedCapacity": "TOTAL"}),
        ),
        (
            "item-collection metrics added on the retry",
            json!({"ReturnConsumedCapacity": "TOTAL"}),
            json!({"ReturnConsumedCapacity": "TOTAL", "ReturnItemCollectionMetrics": "SIZE"}),
        ),
        (
            "item-collection metrics dropped on the retry",
            json!({"ReturnItemCollectionMetrics": "SIZE"}),
            json!({"ReturnItemCollectionMetrics": "NONE"}),
        ),
        (
            "the actions themselves changed",
            json!({"ReturnConsumedCapacity": "TOTAL"}),
            json!({"ReturnConsumedCapacity": "TOTAL"}),
        ),
    ];

    let mut applied = 0i64;
    for (index, (case, first, retried)) in rows.into_iter().enumerate() {
        let token = format!("token-{index}");
        // The first use APPLIES the transaction.
        ok(&router, "TransactWriteItems", transaction(&token, &first, "1")).await;
        applied += 1;
        assert_eq!(
            counter(&router).await,
            applied.to_string(),
            "case {case:?}: the first use of a token applies the transaction"
        );

        // The retry under the same token with a different request is refused, and
        // applies nothing. The last row changes the ACTIONS instead of the reporting
        // members, which is the rule's older half, driven here beside the new one.
        let changed = if case == "the actions themselves changed" {
            transaction(&token, &retried, "2")
        } else {
            transaction(&token, &retried, "1")
        };
        expect_mismatch(&router, case, "TransactWriteItems", changed).await;
        assert_eq!(
            counter(&router).await,
            applied.to_string(),
            "case {case:?}: a refused retry applies nothing"
        );

        // The byte-identical retry is still a REPLAY: served, and applying nothing.
        ok(&router, "TransactWriteItems", transaction(&token, &first, "1")).await;
        assert_eq!(
            counter(&router).await,
            applied.to_string(),
            "case {case:?}: an identical retry re-applies nothing"
        );
    }
}

#[tokio::test]
async fn an_omitted_reporting_member_stands_for_the_level_it_defaults_to() {
    let router = seeded().await;

    // Each row: the members the first call stated and the members the retry stated, both
    // asking for the same thing — the levels that report nothing are what an omitted
    // member means, in either direction and for either member.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "nothing asked for, retried spelling NONE",
            json!({}),
            json!({"ReturnConsumedCapacity": "NONE"}),
        ),
        (
            "NONE asked for, retried omitting the member",
            json!({"ReturnConsumedCapacity": "NONE"}),
            json!({}),
        ),
        (
            "metrics omitted, retried spelling NONE",
            json!({"ReturnConsumedCapacity": "TOTAL"}),
            json!({"ReturnConsumedCapacity": "TOTAL", "ReturnItemCollectionMetrics": "NONE"}),
        ),
    ];

    let mut applied = 0i64;
    for (index, (case, first, retried)) in rows.into_iter().enumerate() {
        let token = format!("same-{index}");
        ok(&router, "TransactWriteItems", transaction(&token, &first, "1")).await;
        applied += 1;
        let replayed = ok(
            &router,
            "TransactWriteItems",
            transaction(&token, &retried, "1"),
        )
        .await;
        assert_eq!(
            counter(&router).await,
            applied.to_string(),
            "case {case:?}: the retry asks for the same thing, so it is a replay that applies \
             nothing, got {replayed}"
        );
    }
}

#[tokio::test]
async fn the_statement_surface_fingerprints_its_reporting_member_too() {
    let router = seeded().await;

    // `ExecuteTransaction` carries the same token and the same capacity member over the
    // statement surface, so the rule holds there too: the first use applies, a retry
    // asking for a different level is refused, and the identical retry is a replay.
    let statements = json!([{
        "Statement": format!("UPDATE {TABLE} SET n = n + 1 WHERE pk = 'ctr'"),
    }]);
    let first = json!({
        "ClientRequestToken": "statement-token",
        "TransactStatements": statements.clone(),
        "ReturnConsumedCapacity": "TOTAL",
    });
    ok(&router, "ExecuteTransaction", first.clone()).await;
    assert_eq!(
        counter(&router).await,
        "1",
        "case the first ExecuteTransaction under a token applies it"
    );

    expect_mismatch(
        &router,
        "ExecuteTransaction retried asking for no capacity",
        "ExecuteTransaction",
        json!({
            "ClientRequestToken": "statement-token",
            "TransactStatements": statements.clone(),
            "ReturnConsumedCapacity": "NONE",
        }),
    )
    .await;
    assert_eq!(
        counter(&router).await,
        "1",
        "case ExecuteTransaction retried asking for no capacity: nothing is applied"
    );

    ok(&router, "ExecuteTransaction", first).await;
    assert_eq!(
        counter(&router).await,
        "1",
        "case the identical ExecuteTransaction retry re-applies nothing"
    );
}
