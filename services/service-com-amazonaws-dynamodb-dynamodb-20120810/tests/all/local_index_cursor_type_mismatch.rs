// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence a paged read states for an `ExclusiveStartKey` whose element carries a type
//! other than the one the table's `AttributeDefinitions` declare for that key attribute.
//!
//! THE RULE: the typed parameter-value sentence — "One or more parameter values were invalid:
//! Type mismatch for Index Key <key> Expected: <declared> Actual: <supplied> IndexName:
//! <index>" — is stated for a key space a GLOBAL secondary index has of its OWN. The base
//! table's key space and a LOCAL secondary index's are one space: a local index "has the same
//! partition key as the table" (`LocalSecondaryIndex$KeySchema`, and
//! `smithy_query(query="shape", shape="LocalSecondaryIndex")`), so its cursor is a key of the
//! table's space with one more of the table's own attributes beside it, and an element of it
//! stated in another type is no key of the space being paged — the invalid TOKEN the base
//! table's own cursor states, "The provided starting key is invalid", with nothing appended.
//!
//! The rows differ along what the rule generalises over: WHICH key space the mistyped element
//! belongs to (the base table's, a local index's own sort key, a global index's partition key
//! and its sort key), WHICH type the element carries instead of the declared one (a number for
//! a string, a string for a number, a set, a map and a boolean), WHICH element of a local
//! index's cursor is mistyped (the index's sort key and each of the table's own key
//! attributes), and the paged read that carries the token (`Query` and `Scan`), because a
//! token is read through the schema being paged whichever read reports it. The control rows
//! page instead of refusing, so the refusals above are about the token's types and not about
//! the request.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// AWS's sentence for a token that is no key of the schema being paged.
const INVALID: &str = "The provided starting key is invalid";

/// AWS's sentence for a GLOBAL index key element stated in another type than the table's
/// `AttributeDefinitions` declare for that attribute, written out as a literal so the WORDING
/// is what these rows are held to.
fn index_key_type_mismatch(attribute: &str, expected: &str, actual: &str, index: &str) -> String {
    format!(
        "One or more parameter values were invalid: Type mismatch for Index Key {attribute} \
         Expected: {expected} Actual: {actual} IndexName: {index}"
    )
}

/// The table every row pages: a composite primary key, a GLOBAL secondary index with a key
/// space of its own, and a LOCAL secondary index sharing the table's partition key.
const TABLE: &str = "CursorKeyTypes";

/// The global secondary index: `gpk` (S) HASH, `gsk` (N) RANGE.
const GLOBAL_INDEX: &str = "ByG";

/// The local secondary index: the table's `pk` HASH, `lsk` (N) RANGE.
const LOCAL_INDEX: &str = "ByLsk";

/// Create [`TABLE`] and seed one item, so a resumed page has somewhere to resume from and a
/// served row is served because the token was a key and not because the table is empty.
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
                {"AttributeName": "lsk", "AttributeType": "N"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": GLOBAL_INDEX,
                "KeySchema": [
                    {"AttributeName": "gpk", "KeyType": "HASH"},
                    {"AttributeName": "gsk", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "LocalSecondaryIndexes": [{
                "IndexName": LOCAL_INDEX,
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
            "gpk": {"S": "OPEN"}, "gsk": {"N": "2"}, "lsk": {"N": "3"},
        }}),
    )
    .await;
    router
}

/// One row: what it is a case of, the index the read pages (`None` for the base table), the
/// partition value a `Query`'s key condition selects in the schema being paged, the token, and
/// the WHOLE sentence that token earns.
struct Row {
    case: &'static str,
    index: Option<&'static str>,
    partition: (&'static str, Value),
    token: Value,
    stated: String,
}

fn rows() -> Vec<Row> {
    let table_partition = ("pk", json!({"S": "one"}));
    let global_partition = ("gpk", json!({"S": "OPEN"}));
    vec![
        // The BASE table's key space: a mistyped element is the invalid token.
        Row {
            case: "the base table, the sort key declared N carried as a string",
            index: None,
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "sk": {"S": "1"}}),
            stated: INVALID.to_string(),
        },
        Row {
            case: "the base table, the partition key declared S carried as a number",
            index: None,
            partition: table_partition.clone(),
            token: json!({"pk": {"N": "1"}, "sk": {"N": "1"}}),
            stated: INVALID.to_string(),
        },
        // The LOCAL index: the same one space, so the same sentence — for its own sort key
        // and for each of the table's key attributes its cursor carries.
        Row {
            case: "a local index, its own sort key declared N carried as a string",
            index: Some(LOCAL_INDEX),
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "lsk": {"S": "3"}, "sk": {"N": "1"}}),
            stated: INVALID.to_string(),
        },
        Row {
            case: "a local index, its own sort key carried as a number set",
            index: Some(LOCAL_INDEX),
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "lsk": {"NS": ["3"]}, "sk": {"N": "1"}}),
            stated: INVALID.to_string(),
        },
        Row {
            case: "a local index, its own sort key carried as a boolean",
            index: Some(LOCAL_INDEX),
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "lsk": {"BOOL": true}, "sk": {"N": "1"}}),
            stated: INVALID.to_string(),
        },
        Row {
            case: "a local index, the table's partition key carried as a number",
            index: Some(LOCAL_INDEX),
            partition: table_partition.clone(),
            token: json!({"pk": {"N": "1"}, "lsk": {"N": "3"}, "sk": {"N": "1"}}),
            stated: INVALID.to_string(),
        },
        Row {
            case: "a local index, the table's sort key carried as a map",
            index: Some(LOCAL_INDEX),
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "lsk": {"N": "3"},
                          "sk": {"M": {"a": {"S": "b"}}}}),
            stated: INVALID.to_string(),
        },
        // The GLOBAL index, whose own key space is reported by the typed sentence.
        Row {
            case: "a global index, its own sort key declared N carried as a string",
            index: Some(GLOBAL_INDEX),
            partition: global_partition.clone(),
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"S": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: index_key_type_mismatch("gsk", "N", "S", GLOBAL_INDEX),
        },
        Row {
            case: "a global index, its own sort key carried as a number set",
            index: Some(GLOBAL_INDEX),
            partition: global_partition.clone(),
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"NS": ["2"]},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: index_key_type_mismatch("gsk", "N", "NS", GLOBAL_INDEX),
        },
        Row {
            case: "a global index, its own partition key declared S carried as a number",
            index: Some(GLOBAL_INDEX),
            partition: global_partition.clone(),
            token: json!({"gpk": {"N": "1"}, "gsk": {"N": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: index_key_type_mismatch("gpk", "S", "N", GLOBAL_INDEX),
        },
        // A global index cursor carrying one of the TABLE's key attributes in another type is
        // not about the index's own space, so it is the invalid token like the rest.
        Row {
            case: "a global index, the table's sort key carried as a string",
            index: Some(GLOBAL_INDEX),
            partition: global_partition,
            token: json!({"gpk": {"S": "OPEN"}, "gsk": {"N": "2"},
                          "pk": {"S": "one"}, "sk": {"S": "1"}}),
            stated: INVALID.to_string(),
        },
    ]
}

/// The request one row sends through `operation`.
fn paged_read(operation: &str, row: &Row) -> Value {
    let mut request = json!({"TableName": TABLE, "ExclusiveStartKey": row.token.clone()});
    if let Some(index) = row.index {
        request["IndexName"] = json!(index);
    }
    if operation == "Query" {
        let (attribute, value) = &row.partition;
        request["KeyConditionExpression"] = json!(format!("{attribute} = :p"));
        request["ExpressionAttributeValues"] = json!({":p": value.clone()});
    }
    request
}

#[tokio::test]
async fn only_a_global_indexs_own_key_space_states_the_typed_cursor_mismatch() {
    let router = seeded().await;

    for row in rows() {
        for operation in ["Query", "Scan"] {
            let case = format!("{} on {operation}", row.case);
            let (status, body) = call(&router, operation, paged_read(operation, &row)).await;
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, "ValidationException", row.stated.as_str()),
                "case {case:?}: expected the refusal {:?}, got {status} with {body}",
                row.stated
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_whose_every_element_carries_its_declared_type_pages_the_read() {
    let router = seeded().await;

    // The other side of the rule: the sentences above are about the TYPES the token carries,
    // so a token whose elements all carry their declared type resumes the read — strictly
    // after the only item, which is the empty page.
    for (case, request) in [
        (
            "the base table's own key",
            json!({"TableName": TABLE,
                   "ExclusiveStartKey": {"pk": {"S": "one"}, "sk": {"N": "1"}}}),
        ),
        (
            "the local index's key beside the table's",
            json!({"TableName": TABLE, "IndexName": LOCAL_INDEX,
                   "ExclusiveStartKey": {"pk": {"S": "one"}, "lsk": {"N": "3"},
                                         "sk": {"N": "1"}}}),
        ),
        (
            "the global index's key beside the table's",
            json!({"TableName": TABLE, "IndexName": GLOBAL_INDEX,
                   "ExclusiveStartKey": {"gpk": {"S": "OPEN"}, "gsk": {"N": "2"},
                                         "pk": {"S": "one"}, "sk": {"N": "1"}}}),
        ),
    ] {
        let page = ok(&router, "Scan", request).await;
        assert_eq!(
            page["Count"],
            json!(0),
            "case {case:?}: the page resumes strictly after the only item, got {page}"
        );
    }
}
