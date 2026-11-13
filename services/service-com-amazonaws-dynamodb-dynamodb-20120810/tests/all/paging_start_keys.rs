// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RESUME TOKEN the paged reads take: `ExclusiveStartKey` on
//! `Query` and on `Scan`.
//!
//! The rules pinned here:
//!
//! 1. a `LastEvaluatedKey` a page reported is a token the next request resumes from, and
//!    the pages it drives partition the matching items with none lost and none repeated;
//! 2. a starting key that is NO key of the schema being paged at all — an attribute the
//!    schema does not declare, an extra attribute beside the key — is refused as the
//!    invalid TOKEN it is: 400 `ValidationException` stating that the starting key is
//!    invalid, and not the bare mismatch AWS words for a `Key` member, which addresses ONE
//!    item and is what `GetItem` and `DeleteItem` carry; a token whose attributes ARE that
//!    schema's key attributes but which states one of them wrongly — another TYPE than the
//!    schema declares, or a declared key attribute left out — states that schema reason
//!    after a colon;
//! 3. the refusal is the same for both paged reads and for both key schemas a table can
//!    declare, and a refused request pages nothing.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and rule 2 is asserted over a
//! TABLE of malformed tokens crossed with the two operations that take one.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// The sentence AWS states for a starting key that is not a key of the schema being
/// paged, as this crate records it.
const START_KEY_INVALID: &str =
    service_com_amazonaws_dynamodb_dynamodb_20120810::state::START_KEY_INVALID_MESSAGE;

/// The sentence AWS states for a starting key whose attributes are that schema's key
/// attributes but which states one of them wrongly — another TYPE than the schema declares,
/// or a declared key attribute left out — which is the defect it diagnoses after the colon.
const START_KEY_TYPE_MISMATCH: &str =
    service_com_amazonaws_dynamodb_dynamodb_20120810::state::START_KEY_SCHEMA_MISMATCH_MESSAGE;

/// The sentence AWS states for a `Key` MEMBER that is not the table's primary key, which
/// the paged reads must NOT borrow.
const KEY_SCHEMA_MISMATCH: &str =
    service_com_amazonaws_dynamodb_dynamodb_20120810::state::KEY_SCHEMA_MISMATCH_MESSAGE;

/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// A table with ONE key attribute, holding `count` items in one partition.
async fn simple_table(router: &MultiServiceRouter, name: &str, count: usize) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    settle().await;
    for position in 0..count {
        ok(
            router,
            "PutItem",
            json!({"TableName": name, "Item": {"Part": {"S": format!("p{position}")}}}),
        )
        .await;
    }
}

/// A table with a COMPOSITE key, holding `count` items in the partition `"one"`.
async fn composite_table(router: &MultiServiceRouter, name: &str, count: usize) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [
                {"AttributeName": "Part", "KeyType": "HASH"},
                {"AttributeName": "Sort", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "Part", "AttributeType": "S"},
                {"AttributeName": "Sort", "AttributeType": "N"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    settle().await;
    for position in 0..count {
        ok(
            router,
            "PutItem",
            json!({"TableName": name,
                   "Item": {"Part": {"S": "one"}, "Sort": {"N": position.to_string()}}}),
        )
        .await;
    }
}

/// One `Query` of the whole `"one"` partition, resumed from `start` when there is one.
async fn query_page(router: &MultiServiceRouter, table: &str, limit: i64, start: &Value) -> Value {
    let mut request = json!({
        "TableName": table,
        "KeyConditionExpression": "Part = :p",
        "ExpressionAttributeValues": {":p": {"S": "one"}},
        "Limit": limit,
    });
    if !start.is_null() {
        request["ExclusiveStartKey"] = start.clone();
    }
    ok(router, "Query", request).await
}

#[tokio::test]
async fn a_last_evaluated_key_resumes_the_page_it_was_reported_on() {
    let router = emulator();
    composite_table(&router, "PagedByToken", 7).await;

    // Rows differ in the page size, so the same seven items are partitioned differently:
    // every page is driven by the token the previous one reported, and what the walk
    // collects is each item exactly once, in key order.
    for (case, limit) in [
        ("one item per page", 1_i64),
        ("two items per page", 2),
        ("three items per page", 3),
        ("every item in one page", 7),
    ] {
        let mut collected: Vec<String> = Vec::new();
        let mut start = Value::Null;
        for _ in 0..16 {
            let page = query_page(&router, "PagedByToken", limit, &start).await;
            let items = page["Items"].as_array().cloned().unwrap_or_default();
            assert!(
                items.len() <= usize::try_from(limit).unwrap_or(0),
                "case {case}: a page holds at most the Limit, got {page}"
            );
            for item in &items {
                collected.push(item["Sort"]["N"].as_str().unwrap_or_default().to_string());
            }
            start = page["LastEvaluatedKey"].clone();
            // A page that reached the Limit reports where it stopped whether or not an
            // item is left to evaluate, so a walk whose last item filled a page asks for
            // one page more: that page is the only empty one, and it is the one that
            // reports no token.
            assert!(
                !items.is_empty() || start.is_null(),
                "case {case}: only the page past the last item is empty, got {page}"
            );
            if start.is_null() {
                break;
            }
        }
        assert_eq!(
            collected,
            (0..7).map(|n| n.to_string()).collect::<Vec<String>>(),
            "case {case}: the pages the tokens drove hold every item once, in key order"
        );
    }
}

#[tokio::test]
async fn a_starting_key_that_is_not_a_key_of_the_schema_is_an_invalid_token() {
    let router = emulator();
    simple_table(&router, "TokensSimple", 3).await;
    composite_table(&router, "TokensComposite", 3).await;

    // Rows differ in HOW the token fails to name a key of the schema, and in the sentence
    // that failure earns: a key attribute carrying another TYPE than the schema declares is
    // the defects AWS diagnoses after the colon, while an attribute the schema does not
    // declare, an extra attribute beside the key and a map carrying no attribute at all
    // each leave the token invalid and nothing more. Each is refused the same way whichever
    // of the two paged reads carries it and whichever key schema the table declares.
    let simple_tokens: Vec<(&str, Value, &str)> = vec![
        (
            "an attribute the schema does not declare",
            json!({"Nope": {"S": "x"}}),
            START_KEY_INVALID,
        ),
        (
            "an extra attribute beside the key",
            json!({"Part": {"S": "p0"}, "Spare": {"S": "x"}}),
            START_KEY_INVALID,
        ),
        (
            "the key attribute as a number",
            json!({"Part": {"N": "1"}}),
            START_KEY_TYPE_MISMATCH,
        ),
        (
            "the key attribute as a set",
            json!({"Part": {"SS": ["p0"]}}),
            START_KEY_TYPE_MISMATCH,
        ),
        ("no attribute at all", json!({}), START_KEY_INVALID),
    ];
    let composite_tokens: Vec<(&str, Value, &str)> = vec![
        (
            "the sort key as a string",
            json!({"Part": {"S": "one"}, "Sort": {"S": "1"}}),
            START_KEY_TYPE_MISMATCH,
        ),
        (
            "a third attribute beside both keys",
            json!({"Part": {"S": "one"}, "Sort": {"N": "1"}, "Spare": {"S": "x"}}),
            START_KEY_INVALID,
        ),
        (
            "only the partition key of a composite schema",
            json!({"Part": {"S": "one"}}),
            START_KEY_TYPE_MISMATCH,
        ),
        (
            "only the sort key of a composite schema",
            json!({"Sort": {"N": "1"}}),
            START_KEY_TYPE_MISMATCH,
        ),
    ];

    for (table, tokens) in [
        ("TokensSimple", simple_tokens),
        ("TokensComposite", composite_tokens),
    ] {
        for (case, token, stated) in tokens {
            for operation in ["Query", "Scan"] {
                let mut request = json!({"TableName": table, "ExclusiveStartKey": token.clone()});
                if operation == "Query" {
                    request["KeyConditionExpression"] = json!("Part = :p");
                    request["ExpressionAttributeValues"] = json!({":p": {"S": "one"}});
                }
                let (status, body) = call(&router, operation, request.clone()).await;
                assert_eq!(
                    (status, error_code(&body).as_str(), message(&body).as_str()),
                    (400, "ValidationException", stated),
                    "case {case:?} on {operation} over {table}: expected the refusal \
                     {stated:?}, got {status} with {body}"
                );
            }
        }
    }

    // A `Key` MEMBER that misses the schema keeps its own wording: the two members are
    // refused in the terms each is stated in.
    let (status, body) = call(
        &router,
        "GetItem",
        json!({"TableName": "TokensSimple", "Key": {"Nope": {"S": "x"}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str(), message(&body).as_str()),
        (400, "ValidationException", KEY_SCHEMA_MISMATCH),
        "case a Key member that misses the schema: it keeps the Key wording, got {status} with \
         {body}"
    );

    // The refusals paged nothing: a valid token still drives the walk afterwards.
    let page = query_page(&router, "TokensComposite", 2, &Value::Null).await;
    let resumed = query_page(&router, "TokensComposite", 2, &page["LastEvaluatedKey"]).await;
    assert_eq!(
        (page["Count"].clone(), resumed["Count"].clone()),
        (json!(2), json!(1)),
        "case after the refusals: the table still pages, got {page} then {resumed}"
    );
}
