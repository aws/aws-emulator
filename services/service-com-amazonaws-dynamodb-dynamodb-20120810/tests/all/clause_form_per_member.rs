// SPDX-License-Identifier: CC0-1.0

//! WHICH form a constraint clause takes is a fact of the SERVED OPERATION.
//!
//! The rule under test: a request member outside the bound its target shape declares is refused
//! 400 `ValidationException` whose clause is worded in the form the served operation gives THAT
//! member — so one member name shared by two operations can be stated differently by each, and
//! two members of one operation can be stated differently too:
//!
//! * the ITEM-REQUEST form names the member by its WIRE spelling and echoes no value (`Value at
//!   'TotalSegments' failed to satisfy constraint: …`, and `Value at 'Limit' …` for the page
//!   size the KEYED read carries);
//! * the REQUEST-VALIDATOR form names it by the lowerCamel path the request validator builds
//!   and echoes the rejected value (`Value '0' at 'limit' failed to satisfy constraint: …`).
//!
//! Nothing about the model chooses between them: the table walk's page size and its two
//! parallel-scan bounds are declared alongside each other on one input — `Scan$Limit` targets
//! `PositiveIntegerObject` `@range{min:1}`, `Scan$TotalSegments`
//! `@range{min:1,max:1000000}` and `Scan$Segment` `@range{min:0,max:999999}`
//! (`smithy_query(query="operation", operation="Scan")`) — and `Query$Limit` targets the very
//! same shape as the walk's page size (`smithy_query(query="operation", operation="Query")`),
//! yet the keyed read states it in the item-request form while the walk states it, and a
//! control-plane listing states the same violation class, in the other form. So the form has to
//! be read
//! off the operation being served and the member the clause is about, which is what the rows
//! below read back.
//!
//! The rows differ along what the rule generalises over: the OPERATION (a table walk, a keyed
//! read, a control-plane listing), the MEMBER (a page size, both parallel-scan bounds), which
//! end of the published bound the value broke, how far past it the value went, and how many
//! members of one request were outside their bounds at once.
//!
//! The rows differ along what the rule generalises over: the OPERATION (a table walk, a keyed
//! read, a control-plane listing), the MEMBER (a page size, both parallel-scan bounds), which
//! end of the published bound the value broke, how far past it the value went, and how many
//! members of one request were outside their bounds at once.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok};

/// The table the rows address: one hash key, which is all a refusal about a bounded numeric
/// member of the request needs.
const TABLE: &str = "ClauseFormPerMember";

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

/// One clause naming `member` by its wire spelling, echoing no value.
fn wire_clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// One clause naming `path` by the validator's path, echoing the rejected `value`.
fn echoing_clause(path: &str, value: i64, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
}

/// The envelope a refusal states `clauses` inside, counting them.
fn envelope(clauses: &[String]) -> String {
    let count = clauses.len();
    let plural = if count == 1 { "error" } else { "errors" };
    format!("{count} validation {plural} detected: {}", clauses.join("; "))
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

/// RULE: the KEYED read states its page size by the member's wire spelling with nothing echoed,
/// whatever value was rejected, while the table WALK and a control-plane listing state the
/// SAME member name breaking the SAME kind of bound with the rejected value echoed under the
/// validator's path.
#[tokio::test]
async fn one_member_name_is_stated_in_the_form_each_operation_gives_it() {
    let router = seeded().await;

    // The values span the rejected side of the one bound every member below declares: zero, one
    // below, a handful below, and the smallest value a 32-bit member carries.
    for rejected in [0_i64, -1, -9, -2_147_483_648] {
        let expected = envelope(&[wire_clause("Limit", &at_least(1))]);
        let case = "a keyed read's page size";
        let stated = refusal(
            &router,
            case,
            "Query",
            json!({"TableName": TABLE, "Limit": rejected,
                   "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?} (Query, Limit {rejected}): expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(&format!("'{rejected}'")),
            "case {case:?} (Query, Limit {rejected}): this form echoes no part of the rejected \
             value, got {stated:?}"
        );

        // The other form, over the same member name and the same broken bound: the table walk
        // and a control-plane listing both echo the rejected value under the validator's path.
        let expected = envelope(&[echoing_clause("limit", rejected, &at_least(1))]);
        for (case, operation, request) in [
            (
                "a table walk's page size",
                "Scan",
                json!({"TableName": TABLE, "Limit": rejected}),
            ),
            (
                "a control-plane listing's page size",
                "ListBackups",
                json!({"Limit": rejected}),
            ),
        ] {
            let stated = refusal(&router, case, operation, request).await;
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}, Limit {rejected}): expected {expected:?}, got \
                 {stated:?}"
            );
        }
    }
}

/// RULE: the walk's OTHER members keep the item-request form — so it states its parallel-scan
/// bounds by their wire spelling at either end of their ranges, and a request outside two of its
/// bounds at once states one clause per member, each in the form that member is given, inside the
/// one envelope that counts them: the page size echoing its value, the parallel-scan bound not.
#[tokio::test]
async fn every_other_member_of_the_walk_keeps_its_operations_form() {
    let router = seeded().await;

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the segment count below its minimum",
            json!({"TableName": TABLE, "TotalSegments": 0}),
            envelope(&[wire_clause("TotalSegments", &at_least(1))]),
        ),
        (
            "the segment count past its maximum",
            json!({"TableName": TABLE, "TotalSegments": 1_000_001}),
            envelope(&[wire_clause("TotalSegments", &at_most(1_000_000))]),
        ),
        (
            "the segment below its zero minimum",
            json!({"TableName": TABLE, "Segment": -1}),
            envelope(&[wire_clause("Segment", &at_least(0))]),
        ),
        (
            "the segment past its maximum",
            json!({"TableName": TABLE, "Segment": 1_000_000}),
            envelope(&[wire_clause("Segment", &at_most(999_999))]),
        ),
        (
            "the page size and the segment count at once",
            json!({"TableName": TABLE, "Limit": 0, "TotalSegments": 0}),
            envelope(&[
                echoing_clause("limit", 0, &at_least(1)),
                wire_clause("TotalSegments", &at_least(1)),
            ]),
        ),
        (
            "the page size and the segment at once",
            json!({"TableName": TABLE, "Limit": -3, "Segment": -1}),
            envelope(&[
                echoing_clause("limit", -3, &at_least(1)),
                wire_clause("Segment", &at_least(0)),
            ]),
        ),
    ];

    for (case, request, expected) in rows {
        let stated = refusal(&router, case, "Scan", request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} (Scan): expected the message {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: the form says nothing about WHEN there is a clause — every member above AT its
/// published bound is inside the range the model declares, so such a request is served.
#[tokio::test]
async fn a_value_at_a_published_bound_is_served_by_either_form() {
    let router = seeded().await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "the walk's page size at its minimum",
            "Scan",
            json!({"TableName": TABLE, "Limit": 1}),
        ),
        (
            "the walk's parallel-scan bounds at their minimum",
            "Scan",
            json!({"TableName": TABLE, "Segment": 0, "TotalSegments": 1}),
        ),
        (
            "the walk's parallel-scan bounds at their maximum",
            "Scan",
            json!({"TableName": TABLE, "Segment": 999_999, "TotalSegments": 1_000_000}),
        ),
        (
            "the keyed read's page size at that same minimum",
            "Query",
            json!({"TableName": TABLE, "Limit": 1, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case {case:?} ({operation}): a request inside every published bound is served, got \
             {status} {body}"
        );
        let stated = message(&body);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {case:?} ({operation}): a value at a published bound is no constraint \
             violation, got {status} {stated:?}"
        );
    }
}
