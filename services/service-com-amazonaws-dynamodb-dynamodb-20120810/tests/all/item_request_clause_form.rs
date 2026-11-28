// SPDX-License-Identifier: CC0-1.0

//! WHICH FORM a constraint clause takes is a fact of the MEMBER of the SERVED OPERATION, over
//! every value that broke a bound.
//!
//! The rule under test: a request member outside the bound its target shape declares is
//! refused 400 `ValidationException`, inside the envelope that counts the clauses, and the
//! clause is worded in the ONE form the served operation gives THAT member:
//!
//! * an ITEM REQUEST — a keyed read, a table walk, a batched or transactional request — names
//!   the member by its WIRE spelling and echoes NO value (`Value at 'Limit' failed to satisfy
//!   constraint: Member must have value greater than or equal to 1`);
//! * a CONTROL-PLANE listing names it by the lowerCamel
//!   path the request validator builds and DOES echo the rejected value (`Value '0' at 'limit'
//!   failed to satisfy constraint: …`), and so does the ONE member of an item request recorded
//!   that way: the table WALK's page size.
//!
//! Nothing in the model distinguishes the two: `Query$Limit` and `Scan$Limit` both target
//! `PositiveIntegerObject` with `@range{min:1}`, `ListBackups$Limit` targets
//! `BackupsInputLimit` with `@range{min:1,max:100}` and `ListTables$Limit` targets
//! `ListTablesInputLimit` with `@range{min:1,max:100}`
//! (`smithy_query(query="operation", operation="Query,Scan,ListBackups,ListTables")`), and the
//! same `RequestItems` map, `@length{min:1}` on both, is stated by its wire spelling on
//! `BatchGetItem` and by the validator's path on `BatchWriteItem`. So the served OPERATION and
//! the member together are the only thing the form can be read off, which is what the rows
//! below read back.
//!
//! The rows span what the rule generalises over: the OPERATION (two item requests, two
//! batched ones, two control-plane listings), the MEMBER (a page size, a parallel-scan bound,
//! a request map), which end of the published bound the value broke, and how far past it the
//! value went — including the integral extremes a 32-bit member carries — so no single row is
//! the specification. The last test is the other side of the same rule: inside the bound there
//! is no clause at all, in either form.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok};

/// The table the item requests address: one hash key, which is all a refusal about a bounded
/// member of the request needs.
const TABLE: &str = "ClauseFormOfTheOperation";

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

/// The requirement phrase AWS states for a `@range` minimum of `min`.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase AWS states for a `@range` maximum of `max`.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The whole message a refusal stating ONE clause carries, over `clause`.
fn envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The message an operation that echoes no value states about `member`.
fn wire_form(member: &str, requirement: &str) -> String {
    envelope(&format!(
        "Value at '{member}' failed to satisfy constraint: {requirement}"
    ))
}

/// The message an operation that echoes the rejected value states about `path`.
fn echoing_form(path: &str, value: i64, requirement: &str) -> String {
    envelope(&format!(
        "Value '{value}' at '{path}' failed to satisfy constraint: {requirement}"
    ))
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

/// The refusal `request` earns, asserted to be a 400 `ValidationException` first so another
/// refusal cannot pass for this one.
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

/// RULE: the KEYED read states the page size it shares with the table walk in the wire form
/// that
/// echoes no value — whatever value was rejected — while the WALK and a control-plane listing,
/// whose page sizes nothing in the model tells apart from it, both take the other form.
#[tokio::test]
async fn an_item_requests_page_size_is_stated_by_its_wire_name_whatever_the_value() {
    let router = seeded().await;

    // The values span the rejected side of the published minimum: zero, one below, a handful
    // below, and the smallest value a 32-bit member carries.
    for rejected in [0_i64, -1, -9, -2_147_483_648] {
        let expected = wire_form("Limit", &at_least(1));
        let case = "a keyed read";
        let stated = refusal(&router, case, "Query", keyed(rejected)).await;
        assert_eq!(
            stated, expected,
            "case {case:?} (Query, Limit {rejected}): expected the message \
             {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(&format!("'{rejected}'")),
            "case {case:?} (Query, Limit {rejected}): expected the rejected value \
             absent from the message, got {stated:?}"
        );

        // The contrast, in the other form: the table walk's page size and a control-plane
        // listing's — the same member name and the same violation class — echo the value
        // under the validator's path.
        let expected = echoing_form("limit", rejected, &at_least(1));
        for (case, operation, request) in [
            ("a table walk", "Scan", walk(rejected)),
            (
                "a control-plane listing",
                "ListBackups",
                json!({"Limit": rejected}),
            ),
        ] {
            let stated = refusal(&router, case, operation, request).await;
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}, Limit {rejected}): expected the \
                 message {expected:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: every other bounded member of an item request takes that same wire form — the walk's
/// parallel-scan bounds at either end of their ranges, and a batched or transactional
/// collection below its own minimum — so the walk's page size is the one member of that input
/// recorded otherwise.
#[tokio::test]
async fn every_member_of_an_item_request_takes_the_same_form() {
    let router = seeded().await;

    // (case, operation, request, the clause the refusal must state).
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "the walk's segment count below its minimum",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 0}),
            wire_form("TotalSegments", &at_least(1)),
        ),
        (
            "the walk's segment count past its maximum",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1_000_001}),
            wire_form("TotalSegments", &at_most(1_000_000)),
        ),
        (
            "the walk's segment below its zero minimum",
            "Scan",
            json!({"TableName": TABLE, "Segment": -1}),
            wire_form("Segment", &at_least(0)),
        ),
        (
            "the walk's segment past its maximum",
            "Scan",
            json!({"TableName": TABLE, "Segment": 1_000_000}),
            wire_form("Segment", &at_most(999_999)),
        ),
        (
            "a batched read's request map below its minimum",
            "BatchGetItem",
            json!({"RequestItems": {}}),
            wire_form("RequestItems", "Member must have length greater than or equal to 1"),
        ),
        (
            "a transactional read's item list below its minimum",
            "TransactGetItems",
            json!({"TransactItems": []}),
            wire_form(
                "TransactItems",
                "Member must have length greater than or equal to 1",
            ),
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
/// validator's path and echoes the rejected value — at either end of the bound, and whatever
/// the value is.
#[tokio::test]
async fn a_control_plane_listing_echoes_the_value_under_the_validators_path() {
    let router = seeded().await;

    // (case, operation, the rejected page size, the clause the refusal must state).
    let rows: Vec<(&str, &str, i64, String)> = vec![
        (
            "a backup listing below its minimum",
            "ListBackups",
            0,
            echoing_form("limit", 0, &at_least(1)),
        ),
        (
            "a backup listing far below its minimum",
            "ListBackups",
            -7,
            echoing_form("limit", -7, &at_least(1)),
        ),
        (
            "a backup listing past its maximum",
            "ListBackups",
            101,
            echoing_form("limit", 101, &at_most(100)),
        ),
        (
            "a table listing below its minimum",
            "ListTables",
            0,
            echoing_form("limit", 0, &at_least(1)),
        ),
        (
            "a table listing past its maximum",
            "ListTables",
            101,
            echoing_form("limit", 101, &at_most(100)),
        ),
    ];

    for (case, operation, limit, expected) in rows {
        let stated = refusal(&router, case, operation, json!({"Limit": limit})).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}, Limit {limit}): expected the message {expected:?}, \
             got {stated:?}"
        );
    }

    // And the batched WRITE, whose request map of the same name and the same bound is stated
    // by the validator's path with no value echoed: the third form, and the reason the form
    // cannot be read off the member.
    let stated = refusal(
        &router,
        "a batched write's request map below its minimum",
        "BatchWriteItem",
        json!({"RequestItems": {}}),
    )
    .await;
    let expected = wire_form(
        "requestItems",
        "Member must have length greater than or equal to 1",
    );
    assert_eq!(
        stated, expected,
        "case \"a batched write's request map below its minimum\": expected the message \
         {expected:?}, got {stated:?}"
    );
}

/// RULE: the form says nothing about WHEN there is a clause — a value the member's bound
/// admits is served, in either form, so what the rows above pin is the bound and not the
/// member being read at all.
#[tokio::test]
async fn a_value_inside_its_bound_states_no_clause_in_either_form() {
    let router = seeded().await;

    let rows: Vec<(&str, &str, Value)> = vec![
        ("a keyed read at its minimum", "Query", keyed(1)),
        ("a table walk at its minimum", "Scan", walk(1)),
        (
            "the walk at both parallel-scan minima",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1, "Segment": 0}),
        ),
        (
            "the walk at both parallel-scan maxima",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1_000_000, "Segment": 999_999}),
        ),
        (
            "a backup listing at its maximum",
            "ListBackups",
            json!({"Limit": 100}),
        ),
        (
            "a table listing at its maximum",
            "ListTables",
            json!({"Limit": 100}),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case {case:?} ({operation}): a request inside every published bound is served, \
             got {status} {body}"
        );
        assert!(
            !message(&body).contains("failed to satisfy constraint"),
            "case {case:?} ({operation}): a value inside its bound is no constraint violation, \
             got {status} {body}"
        );
    }
}
