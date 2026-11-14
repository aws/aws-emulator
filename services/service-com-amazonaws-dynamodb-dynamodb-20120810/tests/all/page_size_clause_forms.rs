// SPDX-License-Identifier: CC0-1.0

//! WHICH form a constraint clause about a numeric member takes, per operation AND per member.
//!
//! One violation class — a value outside the `@range` the model declares, refused 400
//! `ValidationException` inside the envelope that counts the clauses — is stated in two forms
//! by this service:
//!
//! * the ITEM-REQUEST form, which names the member by its WIRE spelling and echoes NO value:
//!   `Value at 'Limit' failed to satisfy constraint: Member must have value greater than or
//!   equal to 1`;
//! * the REQUEST-VALIDATOR form, which names it by the validator's lowerCamel path and DOES
//!   echo the rejected value: `Value '0' at 'limit' failed to satisfy constraint: …`.
//!
//! Nothing about the model chooses between them: `Query$Limit` and `Scan$Limit` both target
//! `PositiveIntegerObject` with `@range{min:1}`
//! (`smithy_query(query="operation", operation="Query,Scan")`), and a keyed read states that
//! member in the item-request form while a table walk states the SAME member in the
//! request-validator form — and states its own parallel-scan bounds, `TotalSegments`
//! (`@range{min:1,max:1000000}`) and `Segment` (`@range{min:0,max:999999}`), in the
//! item-request form beside it. So the form is a fact of the served operation and of the
//! member, which is what these rows read back.
//!
//! The rows differ along what the rule generalises over — the OPERATION (a keyed read, a table
//! walk, a control-plane listing), the MEMBER (a page size, both parallel-scan bounds), which
//! bound was broken (minimum and maximum), how far past it the value went, and the number of
//! violated members in ONE request (one, and two whose forms differ) — so no single row is the
//! specification. The last test pins the other side of each bound: a value AT it is no
//! constraint violation at all.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok};

/// The table every row addresses: one hash key, which is all a refusal about a numeric member
/// of the request needs.
const TABLE: &str = "ClauseForms";

/// The requirement phrase AWS states for a `@range` minimum of `min`.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase AWS states for a `@range` maximum of `max`.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
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

/// The envelope every refusal above states its clauses inside, over `clauses`.
fn envelope(clauses: &[String]) -> String {
    let count = clauses.len();
    let plural = if count == 1 { "error" } else { "errors" };
    format!(
        "{count} validation {plural} detected: {}",
        clauses.join("; ")
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

/// One row: what it is a case of, the operation, the request, and the WHOLE message the
/// refusal must carry.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expected: String,
}

fn rows() -> Vec<Row> {
    let keyed = |limit: i64| {
        json!({"TableName": TABLE, "Limit": limit, "KeyConditionExpression": "pk = :p",
               "ExpressionAttributeValues": {":p": {"S": "one"}}})
    };
    vec![
        Row {
            case: "a keyed read's page size one below its minimum",
            operation: "Query",
            request: keyed(0),
            expected: envelope(&[wire_clause("Limit", &at_least(1))]),
        },
        Row {
            case: "the same member far below the same minimum, through the same read",
            operation: "Query",
            request: keyed(-7),
            expected: envelope(&[wire_clause("Limit", &at_least(1))]),
        },
        Row {
            case: "a table walk's page size one below that same minimum",
            operation: "Scan",
            request: json!({"TableName": TABLE, "Limit": 0}),
            expected: envelope(&[echoing_clause("limit", 0, &at_least(1))]),
        },
        Row {
            case: "the same member far below it, through the same walk",
            operation: "Scan",
            request: json!({"TableName": TABLE, "Limit": -7}),
            expected: envelope(&[echoing_clause("limit", -7, &at_least(1))]),
        },
        Row {
            case: "the walk's segment count one past its maximum",
            operation: "Scan",
            request: json!({"TableName": TABLE, "TotalSegments": 1_000_001}),
            expected: envelope(&[wire_clause("TotalSegments", &at_most(1_000_000))]),
        },
        Row {
            case: "the walk's segment count one below its minimum",
            operation: "Scan",
            request: json!({"TableName": TABLE, "TotalSegments": 0}),
            expected: envelope(&[wire_clause("TotalSegments", &at_least(1))]),
        },
        Row {
            case: "the walk's segment one past its maximum",
            operation: "Scan",
            request: json!({"TableName": TABLE, "Segment": 1_000_000}),
            expected: envelope(&[wire_clause("Segment", &at_most(999_999))]),
        },
        Row {
            case: "the walk's segment one below its zero minimum",
            operation: "Scan",
            request: json!({"TableName": TABLE, "Segment": -1}),
            expected: envelope(&[wire_clause("Segment", &at_least(0))]),
        },
        Row {
            case: "a control-plane listing's page size one below its minimum",
            operation: "ListBackups",
            request: json!({"Limit": 0}),
            expected: envelope(&[echoing_clause("limit", 0, &at_least(1))]),
        },
        Row {
            case: "the same listing's page size one past its maximum",
            operation: "ListBackups",
            request: json!({"Limit": 101}),
            expected: envelope(&[echoing_clause("limit", 101, &at_most(100))]),
        },
        Row {
            // Two violated members of ONE request, whose forms differ: each clause keeps the
            // form of the member it is about, inside one envelope that counts both.
            case: "the walk's page size and its segment count, both outside their bounds",
            operation: "Scan",
            request: json!({"TableName": TABLE, "Limit": 0, "TotalSegments": 0}),
            expected: envelope(&[
                echoing_clause("limit", 0, &at_least(1)),
                wire_clause("TotalSegments", &at_least(1)),
            ]),
        },
    ]
}

/// RULE: a numeric member outside the range the model declares is refused 400
/// `ValidationException` whose clause takes the form the SERVED OPERATION states that MEMBER
/// in — the wire spelling with nothing echoed, or the validator's path with the rejected value
/// — with one clause per violated member inside the envelope that counts them.
#[tokio::test]
async fn a_range_clause_takes_the_form_its_operation_states_that_member_in() {
    let router = seeded().await;

    for Row {
        case,
        operation,
        request,
        expected,
    } in rows()
    {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: a clause in the item-request form echoes NONE of the rejected value, and one in the
/// request-validator form echoes it exactly once — the difference a caller reads the offending
/// value out of.
#[tokio::test]
async fn only_the_request_validator_form_repeats_the_rejected_value() {
    let router = seeded().await;

    // Each row: the operation, the request, the rejected value as the request wrote it, and
    // whether that value belongs in the message.
    let rows: Vec<(&str, &str, Value, &str, bool)> = vec![
        (
            "a keyed read's page size",
            "Query",
            json!({"TableName": TABLE, "Limit": 0, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
            "0",
            false,
        ),
        (
            "a table walk's page size",
            "Scan",
            json!({"TableName": TABLE, "Limit": 0}),
            "0",
            true,
        ),
        (
            "the walk's segment count",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1_000_001}),
            "1000001",
            false,
        ),
        (
            "a control-plane listing's page size",
            "ListBackups",
            json!({"Limit": 101}),
            "101",
            true,
        ),
    ];

    for (case, operation, request, rejected, echoed) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated.contains(&format!("'{rejected}'")),
            echoed,
            "case {case:?} ({operation}): expected the rejected {rejected:?} \
             {} the message, got {stated:?}",
            if echoed { "quoted in" } else { "absent from" }
        );
    }
}

/// RULE: the form a clause takes says nothing about WHEN there is a clause — a value AT a
/// published bound is inside the range the model declares, so it is no constraint violation in
/// either form.
#[tokio::test]
async fn a_value_at_a_published_bound_is_no_constraint_violation() {
    let router = seeded().await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a keyed read's page size at its minimum",
            "Query",
            json!({"TableName": TABLE, "Limit": 1, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        ),
        (
            "a table walk's page size at that same minimum",
            "Scan",
            json!({"TableName": TABLE, "Limit": 1}),
        ),
        (
            "the walk at both parallel-scan bounds' minimum",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1, "Segment": 0}),
        ),
        (
            "the walk at both parallel-scan bounds' maximum",
            "Scan",
            json!({"TableName": TABLE, "TotalSegments": 1_000_000, "Segment": 999_999}),
        ),
        (
            "a control-plane listing's page size at its maximum",
            "ListBackups",
            json!({"Limit": 100}),
        ),
    ];

    for (case, operation, request) in rows {
        let (status, body) = call(&router, operation, request).await;
        // The call is SERVED, so the check cannot pass on a refusal of its own: a body
        // carrying no message at all would satisfy the clause assertion below by itself.
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case {case:?} ({operation}): a request inside every published bound is served, \
             got {status} {body}"
        );
        let stated = message(&body);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {case:?} ({operation}): a value at a published bound is no constraint \
             violation, got {status} {stated:?}"
        );
    }
}
