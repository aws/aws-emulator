// SPDX-License-Identifier: CC0-1.0

//! The TABLE WALK's page size is refused in the request validator's own wording, and its
//! other bounded members are not.
//!
//! RULE: which wording a constraint clause takes is a fact of the SERVED OPERATION and the
//! MEMBER the clause is about, so one member name under one trait is worded differently by two
//! operations and two members of one operation are worded differently too. The table walk's
//! page size is recorded in the REQUEST-VALIDATOR wording — the member named by the lowerCamel
//! path that validator builds, with the rejected value quoted into the sentence (`Value '0' at
//! 'limit' failed to satisfy constraint: Member must have value greater than or equal to 1`) —
//! while the walk's parallel-scan bounds, and the KEYED read's page size of the same name, take
//! the ITEM-REQUEST wording, which names the member as the model spells it on the wire and
//! quotes nothing (`Value at 'Limit' …`).
//!
//! Nothing in the model chooses between the two: `Query$Limit` and `Scan$Limit` are the same
//! member name targeting the same shape under the same trait — `PositiveIntegerObject` with
//! `@range{min:1}` — and `Scan$TotalSegments` (`@range{min:1,max:1000000}`) and `Scan$Segment`
//! (`@range{min:0,max:999999}`) are declared beside the walk's page size on one input
//! (`smithy_query(query="operation", operation="Query,Scan")`), so the wording cannot be read
//! off the member, the trait or the violated bound — only off the operation serving the request
//! together with the member.
//!
//! The rows differ along what the rule generalises over: the rejected VALUE (zero, one below the
//! minimum, a handful below it, and both integral extremes a 32-bit member carries), the
//! OPERATION (the walk, the keyed read, two control-plane listings), the MEMBER (a page size and
//! both parallel-scan bounds), which end of the published bound the value broke, and how many
//! members of ONE request were outside their bounds at once — a request breaking the walk's page
//! size and a parallel-scan bound together states one clause per member, each in its own
//! wording, inside the one envelope that counts them. The last test is the other side of the
//! rule: a value the bound admits earns no clause in either wording.

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok, validation_envelope};

/// The table every row addresses: one hash key, which is all a refusal about a bounded numeric
/// member of the request needs.
const TABLE: &str = "WalkedPageSizeWording";

/// The page sizes the sweep rejects: zero, one below the published minimum, a handful below it,
/// and both integral extremes a 32-bit member can carry below it.
const REJECTED_PAGE_SIZES: [i64; 5] = [0, -1, -9, -2_147_483_647, -2_147_483_648];

/// The requirement phrase AWS states for a `@range` minimum of `min`.
fn at_least(min: i64) -> String {
    format!("Member must have value greater than or equal to {min}")
}

/// The requirement phrase AWS states for a `@range` maximum of `max`.
fn at_most(max: i64) -> String {
    format!("Member must have value less than or equal to {max}")
}

/// One clause in the ITEM-REQUEST wording: the member as the model spells it on the wire, with
/// nothing quoted.
fn wire_clause(member: &str, requirement: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
}

/// One clause in the REQUEST-VALIDATOR wording: the validator's lowerCamel path with the
/// rejected value quoted into the sentence.
fn echoing_clause(path: &str, value: i64, requirement: &str) -> String {
    format!("Value '{value}' at '{path}' failed to satisfy constraint: {requirement}")
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

/// A table walk carrying the page size `limit`.
fn walk(limit: i64) -> Value {
    json!({"TableName": TABLE, "Limit": limit})
}

/// A keyed read carrying the page size `limit`, over a key condition this table answers.
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

/// RULE: the walk's page size below its published minimum is refused with the validator's
/// wording — the lowerCamel member path, with the value the REQUEST carried quoted into the
/// sentence — whatever that value is.
#[tokio::test]
async fn the_walks_page_size_is_refused_with_the_value_the_request_carried() {
    let router = seeded().await;

    for rejected in REJECTED_PAGE_SIZES {
        let expected = validation_envelope(&[echoing_clause("limit", rejected, &at_least(1))]);
        let case = "the walk's page size below its minimum";
        let stated = refusal(&router, case, "Scan", walk(rejected)).await;
        assert_eq!(
            stated, expected,
            "case {case:?} (Scan, Limit {rejected}): expected the message {expected:?}, got \
             {stated:?}"
        );
        // The quoted half is READ OFF the request: the value appears exactly as it was sent,
        // which is what a caller recovers the offending value from.
        assert!(
            stated.contains(&format!("'{rejected}'")),
            "case {case:?} (Scan, Limit {rejected}): expected the rejected value quoted, got \
             {stated:?}"
        );
    }
}

/// RULE: the same member name through the KEYED read, and the walk's OWN other bounded members,
/// keep the item-request wording — the wire spelling with nothing quoted — so the wording above
/// is neither the member's nor the violated bound's.
#[tokio::test]
async fn the_keyed_read_and_the_parallel_scan_bounds_keep_the_wire_wording() {
    let router = seeded().await;

    for rejected in REJECTED_PAGE_SIZES {
        let expected = validation_envelope(&[wire_clause("Limit", &at_least(1))]);
        let case = "the keyed read's page size below the same minimum";
        let stated = refusal(&router, case, "Query", keyed(rejected)).await;
        assert_eq!(
            stated, expected,
            "case {case:?} (Query, Limit {rejected}): expected the message {expected:?}, got \
             {stated:?}"
        );
        assert!(
            !stated.contains(&rejected.to_string()),
            "case {case:?} (Query, Limit {rejected}): this wording quotes no part of the \
             rejected value, got {stated:?}"
        );
    }

    // The walk's own parallel-scan bounds, at either end of each published range: the same
    // operation, the other wording.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the segment count one below its minimum",
            json!({"TableName": TABLE, "TotalSegments": 0}),
            validation_envelope(&[wire_clause("TotalSegments", &at_least(1))]),
        ),
        (
            "the segment count one past its maximum",
            json!({"TableName": TABLE, "TotalSegments": 1_000_001}),
            validation_envelope(&[wire_clause("TotalSegments", &at_most(1_000_000))]),
        ),
        (
            "the segment one below its zero minimum",
            json!({"TableName": TABLE, "Segment": -1}),
            validation_envelope(&[wire_clause("Segment", &at_least(0))]),
        ),
        (
            "the segment one past its maximum",
            json!({"TableName": TABLE, "Segment": 1_000_000}),
            validation_envelope(&[wire_clause("Segment", &at_most(999_999))]),
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

/// RULE: a control-plane listing's page size — the same member name under the same kind of
/// bound — takes the validator's wording too, at either end of its own range, so the walk's
/// wording is the general one and the keyed read's is the recorded exception.
#[tokio::test]
async fn a_control_plane_listing_takes_the_walks_wording() {
    let router = seeded().await;

    // (case, operation, the wire member, its validator path, the value, the requirement).
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
            -9,
            at_least(1),
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
        let expected = validation_envelope(&[echoing_clause(path, rejected, &requirement)]);
        let stated = refusal(&router, case, operation, json!({member: rejected})).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}, {member} {rejected}): expected the message \
             {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: a request outside TWO of the walk's bounds at once states one clause per member, each
/// in that member's own wording, inside the one envelope that counts them — so the two wordings
/// are chosen per member and not per refusal.
#[tokio::test]
async fn one_refusal_states_each_member_in_its_own_wording() {
    let router = seeded().await;

    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the page size and the segment count at once",
            json!({"TableName": TABLE, "Limit": 0, "TotalSegments": 0}),
            validation_envelope(&[
                echoing_clause("limit", 0, &at_least(1)),
                wire_clause("TotalSegments", &at_least(1)),
            ]),
        ),
        (
            "the page size and the segment at once",
            json!({"TableName": TABLE, "Limit": -3, "Segment": -1}),
            validation_envelope(&[
                echoing_clause("limit", -3, &at_least(1)),
                wire_clause("Segment", &at_least(0)),
            ]),
        ),
        (
            "the page size and both parallel-scan bounds at once",
            json!({"TableName": TABLE, "Limit": -1, "Segment": -1,
                   "TotalSegments": 1_000_001}),
            validation_envelope(&[
                echoing_clause("limit", -1, &at_least(1)),
                wire_clause("TotalSegments", &at_most(1_000_000)),
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

/// RULE: the wording says nothing about WHEN there is a clause — a page size the published
/// range admits is served and pages, so what the rows above pin is the broken bound and not the
/// member being read.
#[tokio::test]
async fn a_page_size_inside_its_range_is_served_and_pages() {
    let router = seeded().await;

    for id in ["a", "b", "c"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": {"pk": {"S": id}}}),
        )
        .await;
    }

    // The published minimum, a page size inside the table's own count, and the largest value a
    // 32-bit member carries — `Scan$Limit` declares no maximum, so every one of them is inside
    // its range. A page that reached its page size reports the cursor the next page resumes
    // from, and one that ran out of items first does not.
    for (limit, expected_count, expects_cursor) in [
        (1_usize, 1_usize, true),
        (2, 2, true),
        (3, 3, true),
        (4, 3, false),
        (2_147_483_647, 3, false),
    ] {
        let page = ok(&router, "Scan", json!({"TableName": TABLE, "Limit": limit})).await;
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
