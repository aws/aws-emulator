// SPDX-License-Identifier: CC0-1.0

//! WHERE a page-size refusal echoes the rejected value, the value it echoes is the one the
//! REQUEST carried.
//!
//! A page size below the `@range` minimum the model declares is refused 400
//! `ValidationException` whose clause is stated in one of two forms, per served operation
//! (`tests/all/page_size_clause_forms.rs` pins which form each takes): the
//! request-validator form names the member by its lowerCamel path and quotes the rejected
//! value, the item-request form names it as the model spells it and quotes nothing. This suite
//! is about the QUOTED half of the first form — that the quoted value is read off the request
//! rather than fixed, which is the diagnostic a caller reads the offending value out of.
//!
//! Every row drives the SAME violation class, and they differ along what the rule generalises
//! over: the operation (the two control-plane listings whose page size takes
//! that form, and the table walk whose own page size takes it too), which published bound the
//! value broke (the minimum, and a maximum where the member declares one), how far past it the
//! value went (one past, a handful, and the integral extremes a 32-bit member can carry), and
//! how the rejected value is written (zero, a negative, a large magnitude). The contrast test
//! drives the same sweep through the KEYED read, whose form quotes NO value, so a value
//! that happens to appear in a message cannot pass for a value the clause echoed.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok};

/// The table the walk's rows address: one hash key, which is all a refusal about a numeric
/// member of the request needs.
const TABLE: &str = "EchoedPageSizes";

/// The requirement phrase AWS states for a `@range` minimum of `min`.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase AWS states for a `@range` maximum of `max`.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// The whole message a refusal stating ONE clause in the request-validator form carries, with
/// the rejected value quoted as the request wrote it.
fn echoing(path: &str, value: i64, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
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

/// The page sizes every row below rejects: zero, one below the minimum, a handful below it,
/// and the smallest value a 32-bit integral member can carry.
const REJECTED_BELOW: [i64; 5] = [0, -1, -2, -9, -2_147_483_648];

/// The request a table WALK sends with the page size `limit` — the item read whose page size
/// quotes the rejected value.
fn walk(limit: i64) -> Value {
    json!({"TableName": TABLE, "Limit": limit})
}

/// The request a keyed READ sends with the page size `limit` — the item read whose form quotes
/// no value, over a key condition the table above answers.
fn keyed(limit: i64) -> Value {
    json!({"TableName": TABLE, "Limit": limit, "KeyConditionExpression": "pk = :p",
           "ExpressionAttributeValues": {":p": {"S": "one"}}})
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

/// RULE: a page size outside the range the model declares, on an operation whose clause about
/// that member echoes the rejected value, is refused with THAT value quoted — the one the
/// request carried, written as the request wrote it — under the member's validator path.
#[tokio::test]
async fn an_echoing_page_size_clause_quotes_the_value_the_request_carried() {
    let router = seeded().await;

    for rejected in REJECTED_BELOW {
        // Each row: the case, the operation, its request, and the bound the value broke.
        let rows: Vec<(&str, &str, Value, String)> = vec![
            (
                "a table listing's page size below its minimum",
                "ListTables",
                json!({"Limit": rejected}),
                at_least(1),
            ),
            (
                "a backup listing's page size below its minimum",
                "ListBackups",
                json!({"Limit": rejected}),
                at_least(1),
            ),
            (
                "a table walk's page size below its own minimum",
                "Scan",
                walk(rejected),
                at_least(1),
            ),
        ];
        for (case, operation, request, requirement) in rows {
            let expected = echoing("limit", rejected, &requirement);
            let stated = refusal(&router, case, operation, request).await;
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}, {rejected}): expected the message {expected:?}, \
                 got {stated:?}"
            );
        }
    }

    // The same rule at the other end of a member that declares a MAXIMUM: the value quoted is
    // the one sent, however far past the bound it is.
    for rejected in [101, 1_000, 2_147_483_647] {
        let case = "a backup listing's page size past its maximum";
        let expected = echoing("limit", rejected, &at_most(100));
        let stated = refusal(&router, case, "ListBackups", json!({"Limit": rejected})).await;
        assert_eq!(
            stated, expected,
            "case {case:?} (ListBackups, {rejected}): expected the message {expected:?}, got \
             {stated:?}"
        );
    }
}

/// RULE: the other form quotes NO value — the KEYED read, whose clause about its page size
/// names the member alone, states the same requirement whatever value was rejected, so
/// none of the rejected values appears in the message at all.
#[tokio::test]
async fn a_wire_form_page_size_clause_quotes_none_of_the_rejected_value() {
    let router = seeded().await;

    let expected = format!(
        "1 validation error detected: Value at 'Limit' failed to satisfy constraint: {}",
        at_least(1)
    );
    for rejected in REJECTED_BELOW {
        let case = "a keyed read's page size below its minimum";
        let stated = refusal(&router, case, "Query", keyed(rejected)).await;
        assert_eq!(
            stated, expected,
            "case {case:?} (Query, {rejected}): expected the message {expected:?}, \
             got {stated:?}"
        );
        assert!(
            !stated.contains(&rejected.to_string()),
            "case {case:?} (Query, {rejected}): this form quotes no part of the \
             rejected value, got {stated:?}"
        );
    }
}

/// RULE: the form says nothing about WHEN there is a clause — a page size the member's range
/// admits is served, so the clauses above are the bound and not the member being read at all.
#[tokio::test]
async fn a_page_size_inside_its_range_is_served_by_either_form() {
    let router = seeded().await;

    // The walk's page size declares no maximum (`Scan$Limit` targets `PositiveIntegerObject`
    // with `@range{min:1}`), so even the largest value a 32-bit member carries is inside it.
    let rows: Vec<(&str, &str, Value)> = vec![
        ("the walk at its minimum", "Scan", walk(1)),
        ("the walk far above its minimum", "Scan", walk(2_147_483_647)),
        ("the keyed read at its minimum", "Query", keyed(1)),
        (
            "the backup listing at its maximum",
            "ListBackups",
            json!({"Limit": 100}),
        ),
    ];
    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case {case:?} ({operation}): a page size inside its declared range is served, \
             got {status} {body}"
        );
        let stated = message(&body);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {case:?} ({operation}): a page size inside its range is no constraint \
             violation, got {stated:?}"
        );
    }
}
