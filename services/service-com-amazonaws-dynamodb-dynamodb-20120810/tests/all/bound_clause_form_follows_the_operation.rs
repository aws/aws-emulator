// SPDX-License-Identifier: CC0-1.0

//! WHICH form a constraint clause takes follows the SERVED OPERATION and the MEMBER the clause
//! is about, over every declared bound of that operation's input.
//!
//! The rule under test: a member outside the bound its target shape declares is refused 400
//! `ValidationException`, inside the envelope that counts the clauses, and the clause is worded
//! in the ONE form the served operation gives that member — an ITEM REQUEST (a keyed read, a
//! table walk, a batched or transactional request) names the member by its WIRE spelling and
//! echoes NO value, while a CONTROL-PLANE listing names it by the lowerCamel path the
//! request validator builds and DOES echo the rejected value. One member is recorded on BOTH
//! sides of that line: the page size the item reads SHARE with the listings, which both item
//! reads state in the wire form and a listing in the validator's — and the walk's own
//! parallel-scan bounds stay in the wire form with it.
//!
//! Nothing in the model distinguishes the two forms: `Query$Limit` and `Scan$Limit` target the
//! same shape with the same trait, and `ListBackups$Limit`/`ListTables$Limit` declare the same
//! violation class on a member of the same name
//! (`smithy_query(query="operation", operation="Query,Scan,ListBackups,ListTables,ListExports")`
//! reports `Query$Limit` and `Scan$Limit` targeting `PositiveIntegerObject` `@range{min:1}`,
//! `Scan$TotalSegments` `@range{min:1,max:1000000}`, `Scan$Segment` `@range{min:0,max:999999}`,
//! `ListBackups$Limit` and `ListTables$Limit` `@range{min:1,max:100}` and
//! `ListExports$MaxResults` `@range{min:1,max:25}`; `BatchGetItemInput$RequestItems` and
//! `TransactGetItemsInput$TransactItems` carry `@length` minima of 1).
//!
//! The rows differ along what the rule generalises over: the OPERATION (two item requests, a
//! batched and a transactional one, three control-plane listings), the MEMBER (a page size, both
//! parallel-scan bounds, a request map, an item list), the bound's CLASS (`@range` and
//! `@length`), which end of the published bound the value broke, how far past it the value went
//! (one below, a handful, and the integral extremes a 32-bit member carries), and how many
//! members of one request were outside their bounds at once. The last test is the other side of
//! the same rule: a page size the bound admits is served and pages.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok};

/// The table the item requests address: one hash key, which is all a refusal about a bounded
/// member of the request needs.
const TABLE: &str = "BoundClauseForms";

/// The page sizes below the one minimum `Limit` declares that every row rejects: zero, one
/// below, a handful below, and the smallest value a 32-bit integral member carries.
const REJECTED_PAGE_SIZES: [i64; 4] = [0, -1, -9, -2_147_483_648];

/// The requirement phrase AWS states for a `@range` minimum of `min`.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase AWS states for a `@range` maximum of `max`.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The requirement phrase AWS states for a `@length` minimum of `min`.
fn at_least_long(min: i64) -> String {
    format!("Member must have length greater than or equal to {min}")
}

/// One clause in the ITEM-REQUEST form: the member as the model spells it on the wire, with no
/// value echoed.
fn wire_clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// One clause in the REQUEST-VALIDATOR form: the validator's lowerCamel path with the rejected
/// value echoed.
fn echoing_clause(path: &str, value: i64, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The envelope a refusal states `clauses` inside, counting them.
fn envelope(clauses: &[String]) -> String {
    let count = clauses.len();
    let plural = if count == 1 { "error" } else { "errors" };
    format!("{count} validation {plural} detected: {}", clauses.join("; "))
}

/// Create [`TABLE`] and wait out the `CREATING` -> `ACTIVE` transition the clock owes it.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = crate::emulator::emulator();
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
    describe_when_active(&router, "create the table the rows read", TABLE).await;
    router
}

/// A keyed read carrying the page size `limit`.
fn keyed(limit: i64) -> Value {
    json!({"TableName": TABLE, "Limit": limit, "KeyConditionExpression": "pk = :p",
           "ExpressionAttributeValues": {":p": {"S": "one"}}})
}

/// A table walk carrying the page size `limit`.
fn walk(limit: i64) -> Value {
    json!({"TableName": TABLE, "Limit": limit})
}

/// The message the refusal of `request` carries, asserted to be a 400 `ValidationException`
/// first so a different refusal cannot pass for this one.
async fn refusal(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
    );
    message(&body)
}

/// RULE: BOTH item reads state the page size they share by its wire spelling, with none of
/// the rejected value echoed — whatever the value is.
#[tokio::test]
async fn both_item_requests_state_one_page_size_bound_the_one_way() {
    let router = seeded().await;

    let expected = envelope(&[wire_clause("Limit", &at_least(1))]);
    for rejected in REJECTED_PAGE_SIZES {
        for (case, operation, request) in [
            ("a keyed read's page size", "Query", keyed(rejected)),
            ("a table walk's page size", "Scan", walk(rejected)),
        ] {
            let stated = refusal(&router, case, operation, request).await;
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}, Limit {rejected}): expected the message \
                 {expected:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(&rejected.to_string()),
                "case {case:?} ({operation}, Limit {rejected}): this form echoes no part of \
                 the rejected value, got {stated:?}"
            );
        }
    }
}

/// RULE: every OTHER declared bound of an item request's input is stated in the wire form —
/// the walk's parallel-scan bounds at either end of their ranges, a batched request map and a
/// transactional item list below their `@length` minima — and a request outside two bounds at
/// once states one clause per member, each in the form that member is given, inside the envelope
/// that counts them.
#[tokio::test]
async fn every_other_bound_of_an_item_request_takes_that_same_form() {
    let router = seeded().await;

    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "the walk's segment count one below its minimum",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 0}),
            envelope(&[wire_clause("TotalSegments", &at_least(1))]),
        ),
        (
            "the walk's segment count one past its maximum",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1_000_001}),
            envelope(&[wire_clause("TotalSegments", &at_most(1_000_000))]),
        ),
        (
            "the walk's segment one below its zero minimum",
            "Scan",
            json!({"TableName": TABLE, "Segment": -1}),
            envelope(&[wire_clause("Segment", &at_least(0))]),
        ),
        (
            "the walk's segment one past its maximum",
            "Scan",
            json!({"TableName": TABLE, "Segment": 1_000_000}),
            envelope(&[wire_clause("Segment", &at_most(999_999))]),
        ),
        (
            "a batched read's request map below its length minimum",
            "BatchGetItem",
            json!({"RequestItems": {}}),
            envelope(&[wire_clause("RequestItems", &at_least_long(1))]),
        ),
        (
            "a transactional read's item list below its length minimum",
            "TransactGetItems",
            json!({"TransactItems": []}),
            envelope(&[wire_clause("TransactItems", &at_least_long(1))]),
        ),
        (
            "a transactional write's item list below the same minimum",
            "TransactWriteItems",
            json!({"TransactItems": []}),
            envelope(&[wire_clause("TransactItems", &at_least_long(1))]),
        ),
        (
            "the walk's page size and its segment count at once",
            "Scan",
            json!({"TableName": TABLE, "Limit": 0, "TotalSegments": 0}),
            envelope(&[
                wire_clause("Limit", &at_least(1)),
                wire_clause("TotalSegments", &at_least(1)),
            ]),
        ),
        (
            "the walk's page size and its segment at once",
            "Scan",
            json!({"TableName": TABLE, "Limit": -3, "Segment": -1}),
            envelope(&[
                wire_clause("Limit", &at_least(1)),
                wire_clause("Segment", &at_least(0)),
            ]),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }
}

/// RULE, the other form: a control-plane listing states the same violation class by the
/// validator's lowerCamel path with the rejected value echoed — at either end of its bound, and
/// whatever the value is.
#[tokio::test]
async fn a_control_plane_listing_echoes_the_value_under_the_validators_path() {
    let router = seeded().await;

    let rows: Vec<(&str, &str, &str, &str, i64, String)> = vec![
        (
            "a backup listing below its minimum",
            "ListBackups",
            "Limit",
            "limit",
            0,
            at_least(1),
        ),
        (
            "a backup listing far below its minimum",
            "ListBackups",
            "Limit",
            "limit",
            -9,
            at_least(1),
        ),
        (
            "a backup listing one past its maximum",
            "ListBackups",
            "Limit",
            "limit",
            101,
            at_most(100),
        ),
        (
            "a table listing below its minimum",
            "ListTables",
            "Limit",
            "limit",
            0,
            at_least(1),
        ),
        (
            "a table listing one past its maximum",
            "ListTables",
            "Limit",
            "limit",
            101,
            at_most(100),
        ),
        (
            "an export listing one past its own maximum",
            "ListExports",
            "MaxResults",
            "maxResults",
            26,
            at_most(25),
        ),
    ];

    for (case, operation, member, path, rejected, requirement) in rows {
        let expected = envelope(&[echoing_clause(path, rejected, &requirement)]);
        let stated = refusal(&router, case, operation, json!({member: rejected})).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}, {member} {rejected}): expected the message \
             {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: the form says nothing about WHEN there is a clause — a page size the member's bound
/// admits is served and pages, up to and past the number of items the table holds, so what the
/// rows above pin is the bound and not the member being read at all.
#[tokio::test]
async fn a_page_size_its_bound_admits_is_served_and_pages() {
    let router = seeded().await;

    // The items the walk pages over, so a page size can be below, at and above the count.
    let held = ["a", "b", "c"];
    for id in held {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": {"pk": {"S": id}}}),
        )
        .await;
    }

    // Each row: the page size, how many items the first page holds, and whether that page
    // reports a cursor — a page that reached its Limit reports one even when nothing is left.
    for (limit, expected_count, expects_cursor) in [
        (1_usize, 1_usize, true),
        (2, 2, true),
        (held.len(), held.len(), true),
        (held.len() + 1, held.len(), false),
        (2_147_483_647, held.len(), false),
    ] {
        let page = ok(
            &router,
            "Scan",
            json!({"TableName": TABLE, "Limit": limit}),
        )
        .await;
        assert_eq!(
            page["Count"],
            json!(expected_count),
            "case a walk at Limit {limit}: expected {expected_count} item(s), got {page}"
        );
        assert_eq!(
            !page["LastEvaluatedKey"].is_null(),
            expects_cursor,
            "case a walk at Limit {limit}: expected the cursor {}, got {page}",
            if expects_cursor { "reported" } else { "absent" }
        );
        if expects_cursor {
            // The page the cursor points past: the walk resumes there, and the request that
            // carried a page size the bound admits is served again rather than refused.
            let next = ok(
                &router,
                "Scan",
                json!({"TableName": TABLE, "Limit": limit,
                       "ExclusiveStartKey": page["LastEvaluatedKey"].clone()}),
            )
            .await;
            let remaining = (held.len() - expected_count).min(limit);
            assert_eq!(
                next["Count"],
                json!(remaining),
                "case a walk resumed at Limit {limit}: expected {remaining} item(s) left, got \
                 {next}"
            );
        }
    }

    // And the keyed read at the same minimum, over a key the table holds.
    let read = ok(
        &router,
        "Query",
        json!({"TableName": TABLE, "Limit": 1, "KeyConditionExpression": "pk = :p",
               "ExpressionAttributeValues": {":p": {"S": "a"}}}),
    )
    .await;
    assert_eq!(
        read["Count"],
        json!(1),
        "case a keyed read at Limit 1: expected the one item it keys, got {read}"
    );
}
