// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a paged read states for an `ExclusiveStartKey` that names one of the
//! INDEX's own key attributes in another type than the table's `AttributeDefinitions`
//! declare for it, and WHEN that refusal outranks the token's shape.
//!
//! TWO RULES, one per half of this suite.
//!
//! 1. The wording follows the READ the token was handed to. A table WALK details the
//!    mismatch — "One or more parameter values were invalid: Type mismatch for Index Key
//!    <key> Expected: <declared> Actual: <supplied> IndexName: <index>", with the key
//!    attribute, the type the table declares for it, the type the token supplied and the
//!    index named from the request and the table's stored schema — while a QUERY states the
//!    one fixed invalid-token sentence, "The provided starting key is invalid", for every
//!    token it cannot place in the schema it pages, that element included. Every element of
//!    an index cursor is held to its declared type that way: the index's own key attributes
//!    and the key attributes of the TABLE the cursor carries beside them, because that cursor
//!    is a key of the index's order as a whole and the sentence names the index the request
//!    addressed.
//!
//! 2. That mismatch is decided BEFORE the token is held to the shape of the schema being
//!    paged: an element stated in a type the index cannot hold is that invalid parameter
//!    value whatever else the token carries, so a cursor that is also incomplete, or that
//!    carries a spare attribute, still names the element and the two types. A token whose
//!    every index key element carries its declared type is held to the shape as before.
//!
//! The rows span what the rules generalise over: WHICH element of the index key schema is
//! mistyped (the partition key and the sort key), WHICH type it carries instead of the
//! declared one (a string for a number, a number for a string, a string for a binary, a
//! boolean, a map, a list, a null and a well-formed set), WHAT ELSE is wrong with the same
//! token (nothing, a key attribute left out, a spare attribute, both), WHICH schema declares
//! the element (a global index's own, a local index's own, and the table's alone) and the read
//! that carries it (`Query` and `Scan`) — and the control rows page instead of refusing, so
//! what is pinned is the token's element and not the call.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// AWS's fixed sentence for a token the read cannot place in the schema it pages.
const INVALID: &str = "The provided starting key is invalid";

/// AWS's sentence for an index key element stated in another type than the table's
/// `AttributeDefinitions` declare for it, written out as a literal so the WORDING is what
/// these rows are held to.
fn type_mismatch(attribute: &str, expected: &str, actual: &str, index: &str) -> String {
    format!(
        "One or more parameter values were invalid: Type mismatch for Index Key {attribute} \
         Expected: {expected} Actual: {actual} IndexName: {index}"
    )
}

/// The table every row pages: a composite primary key, a GLOBAL secondary index with a key
/// space of its own over a string and a number, a second global one keyed on a BINARY
/// attribute, and a LOCAL one sharing the table's partition key.
const TABLE: &str = "CursorElementTypes";

/// The composite global secondary index: `gpk` (S) HASH, `gsk` (N) RANGE.
const GLOBAL: &str = "ByGpkGsk";

/// The binary-keyed global secondary index: `bpk` (B) HASH.
const BINARY: &str = "ByBpk";

/// The local secondary index: the table's `pk` HASH, `lsk` (N) RANGE.
const LOCAL: &str = "ByLsk";

/// Create [`TABLE`] and seed one item, so a token that IS a key has somewhere to resume from
/// and the refusals below are about the token rather than about an empty table.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "gpk", "AttributeType": "S"},
                {"AttributeName": "gsk", "AttributeType": "N"},
                {"AttributeName": "bpk", "AttributeType": "B"},
                {"AttributeName": "lsk", "AttributeType": "N"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": GLOBAL,
                    "KeySchema": [
                        {"AttributeName": "gpk", "KeyType": "HASH"},
                        {"AttributeName": "gsk", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": BINARY,
                    "KeySchema": [{"AttributeName": "bpk", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
            "LocalSecondaryIndexes": [{
                "IndexName": LOCAL,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "lsk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    describe_when_active(&router, "create the table every row pages", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {
            "pk": {"S": "one"}, "sk": {"N": "1"},
            "gpk": {"S": "OPEN"}, "gsk": {"N": "2"},
            "bpk": {"B": "YQ=="}, "lsk": {"N": "3"},
        }}),
    )
    .await;
    router
}

/// The partition value a `Query`'s key condition selects in the schema being paged, named for
/// the index each row reads.
fn partition_of(index: &str) -> (&'static str, Value) {
    match index {
        GLOBAL => ("gpk", json!({"S": "OPEN"})),
        BINARY => ("bpk", json!({"B": "YQ=="})),
        _ => ("pk", json!({"S": "one"})),
    }
}

/// The request one row sends through `operation`, paging `index` from `token`.
fn paged_read(operation: &str, index: &str, token: &Value) -> Value {
    let mut request = json!({
        "TableName": TABLE,
        "IndexName": index,
        "ExclusiveStartKey": token.clone(),
    });
    if operation == "Query" {
        let (attribute, value) = partition_of(index);
        request["KeyConditionExpression"] = json!(format!("{attribute} = :p"));
        request["ExpressionAttributeValues"] = json!({":p": value});
    }
    request
}

/// The refusal `request` earns, held to the class first so another refusal cannot pass for
/// this one.
async fn refusal(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} with {body}"
    );
    message(&body)
}

/// One row of RULE 1: the index paged, the token, and the sentence the table WALK states for
/// it — the `Query` states [`INVALID`] for every one of them.
struct Row {
    case: &'static str,
    index: &'static str,
    token: Value,
    walked: String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a global index's sort key declared N carried as a string",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"S": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gsk", "N", "S", GLOBAL),
        },
        Row {
            case: "a global index's partition key declared S carried as a number",
            index: GLOBAL,
            token: json!({"gpk": {"N": "7"}, "gsk": {"N": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gpk", "S", "N", GLOBAL),
        },
        Row {
            case: "a global index's sort key carried as a boolean",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"BOOL": true},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gsk", "N", "BOOL", GLOBAL),
        },
        Row {
            case: "a global index's sort key carried as a map",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"M": {"a": {"S": "b"}}},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gsk", "N", "M", GLOBAL),
        },
        Row {
            case: "a global index's sort key carried as a list",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"L": [{"N": "2"}]},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gsk", "N", "L", GLOBAL),
        },
        Row {
            case: "a global index's sort key carried as a null",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"NULL": true},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gsk", "N", "NULL", GLOBAL),
        },
        Row {
            case: "a global index's sort key carried as a well-formed number set",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"NS": ["2", "3"]},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("gsk", "N", "NS", GLOBAL),
        },
        Row {
            case: "a global index's binary partition key carried as a string",
            index: BINARY,
            token: json!({"bpk": {"S": "a"}, "pk": {"S": "one"}, "sk": {"N": "1"}}),
            walked: type_mismatch("bpk", "B", "S", BINARY),
        },
        // A LOCAL index's cursor is held to the same rule, over its own sort key and over the
        // table's key attributes it carries.
        Row {
            case: "a local index's own sort key declared N carried as a string",
            index: LOCAL,
            token: json!({"pk": {"S": "one"}, "lsk": {"S": "3"}, "sk": {"N": "1"}}),
            walked: type_mismatch("lsk", "N", "S", LOCAL),
        },
        Row {
            case: "a local index's own sort key carried as a boolean",
            index: LOCAL,
            token: json!({"pk": {"S": "one"}, "lsk": {"BOOL": false}, "sk": {"N": "1"}}),
            walked: type_mismatch("lsk", "N", "BOOL", LOCAL),
        },
        Row {
            case: "a local index cursor whose table sort key is carried as a string",
            index: LOCAL,
            token: json!({"pk": {"S": "one"}, "lsk": {"N": "3"}, "sk": {"S": "1"}}),
            walked: type_mismatch("sk", "N", "S", LOCAL),
        },
        // One of the TABLE's key attributes carried in another type is about the table's key
        // space and not about the index's own, so it is the invalid token too.
        Row {
            case: "a global index cursor whose table sort key is carried as a string",
            index: GLOBAL,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"N": "2"},
                          "pk": {"S": "one"}, "sk": {"S": "1"}}),
            walked: type_mismatch("sk", "N", "S", GLOBAL),
        },
    ]
}

#[tokio::test]
async fn each_read_states_its_own_wording_for_a_mistyped_index_key_element() {
    let router = seeded().await;

    for row in rows() {
        for (operation, expected) in [
            // The QUERY states the fixed sentence for every token it cannot place.
            ("Query", INVALID.to_string()),
            ("Scan", row.walked.clone()),
        ] {
            let case = format!("{} on {operation}", row.case);
            let stated = refusal(
                &router,
                &case,
                operation,
                paged_read(operation, row.index, &row.token),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected the refusal {expected:?}, got {stated:?}"
            );
            // The fixed sentence is fixed text: nothing the request named reaches it.
            if expected == INVALID {
                for named in ["pk", "sk", "gpk", "gsk", "bpk", "lsk", GLOBAL, BINARY, LOCAL] {
                    assert!(
                        !stated.contains(named),
                        "case {case:?}: the fixed sentence must not name {named:?}, got \
                         {stated:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn the_mistyped_element_outranks_everything_else_wrong_with_the_token() {
    let router = seeded().await;

    // RULE 2: the same mistyped element, in tokens that are ALSO wrong in another way — an
    // index key attribute left out, one of the table's left out, a spare attribute the schema
    // declares no key for, and all of that at once. The walk still names the element and the
    // two types; the query still states its one sentence.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "the mistyped sort key with the table's sort key left out",
            json!({"gpk": {"S": "OPEN"}, "gsk": {"S": "2"}, "pk": {"S": "one"}}),
            type_mismatch("gsk", "N", "S", GLOBAL),
        ),
        (
            "the mistyped sort key with the index key alone, both table keys left out",
            json!({"gpk": {"S": "OPEN"}, "gsk": {"S": "2"}}),
            type_mismatch("gsk", "N", "S", GLOBAL),
        ),
        (
            "the mistyped partition key beside a spare attribute",
            json!({"gpk": {"N": "7"}, "gsk": {"N": "2"}, "pk": {"S": "one"},
                   "sk": {"N": "1"}, "spare": {"S": "x"}}),
            type_mismatch("gpk", "S", "N", GLOBAL),
        ),
        (
            "the mistyped sort key beside a spare attribute and a key left out",
            json!({"gpk": {"S": "OPEN"}, "gsk": {"S": "2"}, "spare": {"S": "x"}}),
            type_mismatch("gsk", "N", "S", GLOBAL),
        ),
    ];

    for (case, token, detailed) in rows {
        for (operation, expected) in [
            ("Query", INVALID.to_string()),
            ("Scan", detailed.clone()),
        ] {
            let case = format!("{case} on {operation}");
            let stated = refusal(
                &router,
                &case,
                operation,
                paged_read(operation, GLOBAL, &token),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected the refusal {expected:?}, got {stated:?}"
            );
        }
    }

    // The other side of RULE 2: a token whose every INDEX key element carries its declared
    // type is held to the shape of the schema being paged, so it earns the fixed sentence on
    // either read however it is malformed.
    let shaped: Vec<(&str, Value)> = vec![
        (
            "the index key alone, both table keys left out",
            json!({"gpk": {"S": "OPEN"}, "gsk": {"N": "2"}}),
        ),
        (
            "a spare attribute beside the whole cursor",
            json!({"gpk": {"S": "OPEN"}, "gsk": {"N": "2"}, "pk": {"S": "one"},
                   "sk": {"N": "1"}, "spare": {"S": "x"}}),
        ),
        (
            "the index sort key left out",
            json!({"gpk": {"S": "OPEN"}, "pk": {"S": "one"}, "sk": {"N": "1"}}),
        ),
        ("no attribute at all", json!({})),
    ];
    for (case, token) in shaped {
        for operation in ["Query", "Scan"] {
            let case = format!("{case} on {operation}");
            let stated = refusal(
                &router,
                &case,
                operation,
                paged_read(operation, GLOBAL, &token),
            )
            .await;
            assert_eq!(
                stated, INVALID,
                "case {case:?}: expected exactly {INVALID:?}, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_whose_elements_carry_their_declared_types_pages_either_read() {
    let router = seeded().await;

    // The control: the cursor each read reports for the one stored item resumes the read
    // strictly after it, which is the empty page — so every refusal above is about the type
    // an element carried and not about paging an index at all.
    for index in [GLOBAL, BINARY, LOCAL] {
        let token = match index {
            GLOBAL => json!({"gpk": {"S": "OPEN"}, "gsk": {"N": "2"},
                             "pk": {"S": "one"}, "sk": {"N": "1"}}),
            BINARY => json!({"bpk": {"B": "YQ=="}, "pk": {"S": "one"}, "sk": {"N": "1"}}),
            _ => json!({"pk": {"S": "one"}, "lsk": {"N": "3"}, "sk": {"N": "1"}}),
        };
        for operation in ["Query", "Scan"] {
            let case = format!("the cursor of {index} on {operation}");
            let page = ok(&router, operation, paged_read(operation, index, &token)).await;
            assert_eq!(
                page["Count"],
                json!(0),
                "case {case:?}: the page resumes strictly after the only item, got {page}"
            );
        }
    }
}
