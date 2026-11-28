// SPDX-License-Identifier: CC0-1.0

//! What a request whose `TableName` states NO NAME is refused with.
//!
//! THE RULE: the table name is the whole selector a request addresses a table by, so a value
//! too short for the member to name one — the empty string, the only value below the
//! `@length` minimum of 1 the member's target declares (`smithy_query(query="operation",
//! operation="UpdateItem,PutItem,GetItem,Scan")` gives `TableName` `@required`, target
//! `TableArn`, `@length` 1..1024) — leaves the request pointed at nothing. It is refused HTTP
//! 400 `ValidationException` whose whole message is ONE clause: "1 validation error detected:
//! Value at 'TableName' failed to satisfy constraint: Member must have length greater than or
//! equal to 1". Two things are part of that: the member is named as the API spells it, with no
//! rendered value where the submitted name would stand, and nothing else wrong with the same
//! request is reported beside it — the remaining field checks are not reached at all.
//!
//! The rows span what the rule generalises over: the OPERATION (the three single-item writes,
//! the single-item read, both paged reads and two control-plane calls), and WHAT ELSE the same
//! request breaks (an enumerated member outside its set, a second such member, a page size
//! below its range minimum, a projection member outside its set, and nothing at all) —
//! because none of that is reported.
//!
//! Two boundaries of the same rule are read back here. A request the READER could not build —
//! one that states no value for a member its input requires — is a different refusal: that
//! reading reports every bound the body broke, the absent member among them, in the envelope
//! that counts them. And a name INSIDE the bound is no constraint violation at all, whether
//! or not a table answers to it.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};

use crate::emulator::{absent_clause, call, emulator, error_code, message, ok, validation_envelope};
use crate::table_items::create_active_table;

/// The whole message a request that names no table earns, written out as a literal so the
/// WORDING is what these rows are held to.
const NAMES_NO_TABLE: &str = "1 validation error detected: Value at 'TableName' failed to \
                              satisfy constraint: Member must have length greater than or \
                              equal to 1";

/// A table that DOES exist, so the control rows are served rather than refused.
const TABLE: &str = "SelectorBound";

/// The key attribute of that table.
const KEY: &str = "pk";

/// That table's key, as a `Key` map.
fn key() -> Value {
    json!({KEY: {"S": "one"}})
}

/// The rows: the case, the operation, and the request — each naming no table, and each
/// breaking something else the refusal must not report.
fn rows() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "a write beside an out-of-set ReturnValues",
            "PutItem",
            json!({"TableName": "", "Item": {KEY: {"S": "one"}},
                   "ReturnValues": "BOGUS_RV"}),
        ),
        (
            "an update beside an out-of-set ReturnValues",
            "UpdateItem",
            json!({"TableName": "", "Key": key(),
                   "UpdateExpression": "SET note = :v",
                   "ExpressionAttributeValues": {":v": {"S": "x"}},
                   "ReturnValues": "BOGUS_RV"}),
        ),
        (
            "an update beside two out-of-set members at once",
            "UpdateItem",
            json!({"TableName": "", "Key": key(),
                   "ReturnValues": "BOGUS_RV", "ReturnConsumedCapacity": "BOGUS_RCC"}),
        ),
        (
            "a delete beside an out-of-set ReturnConsumedCapacity",
            "DeleteItem",
            json!({"TableName": "", "Key": key(), "ReturnConsumedCapacity": "BOGUS_RCC"}),
        ),
        (
            "a keyed read beside an out-of-set ReturnConsumedCapacity",
            "GetItem",
            json!({"TableName": "", "Key": key(), "ReturnConsumedCapacity": "BOGUS_RCC"}),
        ),
        (
            "a query beside a page size below its minimum",
            "Query",
            json!({"TableName": "", "Limit": 0,
                   "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        ),
        (
            "a table walk beside a page size below its minimum and an out-of-set Select",
            "Scan",
            json!({"TableName": "", "Limit": 0, "Select": "BOGUS_SELECT"}),
        ),
        (
            "a table walk with nothing else wrong",
            "Scan",
            json!({"TableName": ""}),
        ),
        (
            "a description with nothing else wrong",
            "DescribeTable",
            json!({"TableName": ""}),
        ),
        (
            "a destruction with nothing else wrong",
            "DeleteTable",
            json!({"TableName": ""}),
        ),
    ]
}

#[tokio::test]
async fn a_request_that_names_no_table_is_refused_by_that_member_alone() {
    let router = emulator();
    create_active_table(&router, TABLE, &[(KEY, "HASH", "S")]).await;

    for (case, operation, request) in rows() {
        let (status, body) = call(&router, operation, request).await;
        let stated = message(&body);
        assert_eq!(
            (status, error_code(&body).as_str(), stated.as_str()),
            (400, "ValidationException", NAMES_NO_TABLE),
            "case {case:?} ({operation}): expected the refusal {NAMES_NO_TABLE:?}, got {status} \
             with {body}"
        );
        // Neither the lowerCamel path the request validator builds, nor a rendered value
        // where the submitted name would stand, nor any other member of the same request.
        for absent in [
            "'tableName'",
            "Value ''",
            "returnValues",
            "ReturnValues",
            "returnConsumedCapacity",
            "Limit",
            "Select",
            "2 validation",
        ] {
            assert!(
                !stated.contains(absent),
                "case {case:?} ({operation}): the refusal must not state {absent:?}, got \
                 {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_the_reader_could_not_build_reports_every_bound_it_broke() {
    let router = emulator();

    // The first boundary: a member the input REQUIRES and the body does not carry stops the
    // reading of the request, and what is reported is that reading's own account of every
    // bound the body broke — the empty name among them, in the request validator's form, and
    // the absent member beside it.
    let empty_name = "Value '' at 'tableName' failed to satisfy constraint: Member must have \
                      length greater than or equal to 1";
    let capacity = "Value 'BOGUS_RCC' at 'returnConsumedCapacity' failed to satisfy constraint: \
                    Member must satisfy enum value set: [INDEXES, TOTAL, NONE]";
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "an update with no key, beside the empty name",
            "UpdateItem",
            json!({"TableName": "", "ReturnConsumedCapacity": "BOGUS_RCC"}),
            validation_envelope(&[
                capacity.to_string(),
                empty_name.to_string(),
                absent_clause("key"),
            ]),
        ),
        (
            "a keyed read with no key, beside the empty name",
            "GetItem",
            json!({"TableName": "", "ReturnConsumedCapacity": "BOGUS_RCC"}),
            validation_envelope(&[
                capacity.to_string(),
                empty_name.to_string(),
                absent_clause("key"),
            ]),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body)),
            (400, "ValidationException", expected.clone()),
            "case {case:?} ({operation}): expected {expected:?}, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn a_name_inside_the_bound_is_no_constraint_violation() {
    let router = emulator();
    create_active_table(&router, TABLE, &[(KEY, "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {KEY: {"S": "one"}}}),
    )
    .await;

    // The second boundary: a name the member's bound admits is read as the name it is, so the
    // request is answered about the TABLE — not found for a name no table answers to, and
    // served for the one that does. Either way nothing about `TableName` failed a constraint,
    // which is what makes the rows above about the bound and not about the member being read.
    for (case, operation, name, request) in [
        (
            "a keyed read of a name no table answers to",
            "GetItem",
            "abc",
            json!({"Key": key()}),
        ),
        (
            "a table walk of a name no table answers to",
            "Scan",
            "abc",
            json!({}),
        ),
    ] {
        let mut body = request;
        body["TableName"] = json!(name);
        let (status, answered) = call(&router, operation, body).await;
        let stated = message(&answered);
        assert!(
            !stated.contains("failed to satisfy constraint"),
            "case {case:?} ({operation}): a name inside the bound breaks no constraint, got \
             {status} with {answered}"
        );
    }

    // And the table that does exist is served, through both the keyed read and the walk.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key()}),
    )
    .await;
    assert_eq!(
        read["Item"][KEY]["S"],
        json!("one"),
        "case \"the stored item\": expected the item the write stored, got {read}"
    );
    let walked = ok(&router, "Scan", json!({"TableName": TABLE})).await;
    assert_eq!(
        walked["Count"],
        json!(1),
        "case \"the stored item\": expected the walk to report it, got {walked}"
    );
}
