// SPDX-License-Identifier: CC0-1.0

//! What the index named in a PartiQL statement's `FROM` qualifier RESOLVES to, on a table
//! that carries a vector index beside its secondary ones.
//!
//! The rule pinned here: a name the statement's source qualifier carries is resolved against
//! the indexes the table declares, and a name that resolves to an index of a kind no
//! statement can be served from is refused as that index TYPE — a vector index is SEARCHED
//! (`SearchVectors` "performs a vector similarity search on a vector index") and declares no
//! primary key schema a `WHERE` clause could pin, so every statement over it asks for a walk
//! of the whole index, which that kind of index does not serve. Only a name the table
//! carries NO index for keeps the sentence for an unresolved source qualifier: the table
//! does have the vector index, so reporting it absent would send a caller creating an index
//! that is already there.
//!
//! The rows span what the rule generalises over: the KIND of index the qualifier names (a
//! vector index, a global secondary index, a local secondary index, a name no index of the
//! table answers to) and the SHAPE of the statement over it (a whole-source walk, a `WHERE`
//! clause pinning the table's partition key, a clause over another attribute, a projection
//! instead of `*`) — the kind the name resolves to decides the answer, and the clause does
//! not. The table itself, read with no qualifier at all, is the positive control beside
//! them, and `SearchVectors` over the same index is the reader a vector index IS for.
//!
//! Every row is driven over the wire (`POST /` with `X-Amz-Target:
//! DynamoDB_20120810.<OperationName>`) through the `MultiServiceRouter` the emulator's front
//! doors use.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok, table_when, REGION};

/// The table every row reads.
const TABLE: &str = "PartiqlVectorIndexSource";

/// The vector index of that table, and the two secondary indexes beside it.
const VECTOR_INDEX: &str = "vec-idx";
const GLOBAL_INDEX: &str = "gsi-idx";
const LOCAL_INDEX: &str = "lsi-idx";

/// AWS's fixed sentence for a statement whose source qualifier names an index of a kind no
/// statement reads: it names the read that was asked for and the index TYPE, and nothing of
/// the index or of the table.
const WRONG_TYPE: &str = "Scan operation not supported on this index type";

/// AWS's fixed sentence for a statement whose source qualifier names no index of the table:
/// the sentence alone, naming nothing of what the table does carry.
const UNRESOLVED: &str = "The table does not have the specified index";

/// Create the table — a composite primary key, a global and a local secondary index, and
/// one vector index over `emb` — wait for the table AND its vector index to reach `ACTIVE`,
/// and store the one item that reaches every index.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "tenant", "AttributeType": "S"},
                {"AttributeName": "rank", "AttributeType": "N"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": GLOBAL_INDEX,
                "KeySchema": [{"AttributeName": "tenant", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "LocalSecondaryIndexes": [{
                "IndexName": LOCAL_INDEX,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "rank", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "VectorIndexes": [{
                "IndexName": VECTOR_INDEX,
                "VectorAttribute": {"AttributeName": "emb"},
                "Dimensions": 2,
                "DistanceFunction": "COSINE",
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    // The vector index the rows name is ACTIVE before any of them runs, so a refusal that
    // reported it absent would contradict the table's own description. Waited for as a
    // CONDITION of the description rather than for a span of the clock.
    let described = table_when(&router, "the seeded table", REGION, TABLE, |table| {
        table["TableStatus"] == json!("ACTIVE")
            && table["VectorIndexes"][0]["IndexStatus"] == json!("ACTIVE")
    })
    .await;
    assert_eq!(
        described["VectorIndexes"][0]["IndexName"],
        json!(VECTOR_INDEX),
        "the table reports the vector index the rows name, got {described}"
    );
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {
                "pk": {"S": "i1"},
                "sk": {"N": "1"},
                "tenant": {"S": "t1"},
                "rank": {"N": "5"},
                "emb": {"L": [{"N": "1"}, {"N": "0"}]},
            },
        }),
    )
    .await;
    router
}

/// What a statement naming one index is answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The qualifier named an index a statement is served from.
    Served,
    /// The qualifier resolved to an index of a kind no statement reads.
    WrongType,
    /// The table carries no index by that name.
    Unresolved,
}

#[tokio::test]
async fn a_statement_naming_a_vector_index_is_refused_as_that_index_type() {
    let router = seeded().await;

    let rows: [(&str, String, Expected); 7] = [
        (
            "a vector index walked whole",
            format!("SELECT * FROM \"{TABLE}\".\"{VECTOR_INDEX}\""),
            Expected::WrongType,
        ),
        (
            "a vector index with the table's partition key pinned",
            format!("SELECT * FROM \"{TABLE}\".\"{VECTOR_INDEX}\" WHERE pk = 'i1'"),
            Expected::WrongType,
        ),
        (
            "a vector index under a projection and another clause",
            format!("SELECT emb FROM \"{TABLE}\".\"{VECTOR_INDEX}\" WHERE tenant = 't1'"),
            Expected::WrongType,
        ),
        (
            "a global secondary index",
            format!("SELECT * FROM \"{TABLE}\".\"{GLOBAL_INDEX}\" WHERE tenant = 't1'"),
            Expected::Served,
        ),
        (
            "a local secondary index",
            format!("SELECT * FROM \"{TABLE}\".\"{LOCAL_INDEX}\" WHERE pk = 'i1'"),
            Expected::Served,
        ),
        (
            "no index of the table",
            format!("SELECT * FROM \"{TABLE}\".\"nowhere-idx\""),
            Expected::Unresolved,
        ),
        (
            "the table itself",
            format!("SELECT * FROM \"{TABLE}\" WHERE pk = 'i1'"),
            Expected::Served,
        ),
    ];

    for (case, statement, expected) in rows {
        let (status, body) = call(&router, "ExecuteStatement", json!({"Statement": statement})).await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the statement to be served, got {status} with {body}"
                );
                assert_eq!(
                    body["Items"].as_array().map(Vec::len).unwrap_or_default(),
                    1,
                    "case {case:?}: expected the one stored item, got {body}"
                );
            }
            Expected::WrongType | Expected::Unresolved => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the statement to be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: the name the statement carried is its own defect, got {body}"
                );
                let sentence = if expected == Expected::WrongType {
                    WRONG_TYPE
                } else {
                    UNRESOLVED
                };
                assert_eq!(
                    message(&body),
                    sentence,
                    "case {case:?}: expected the sentence for that kind of name, got {body}"
                );
            }
        }
    }

    // ...and the vector index serves the reader it IS for, so every refusal above is about
    // which reader was asked and not about whether the index is there.
    let searched = ok(
        &router,
        "SearchVectors",
        json!({
            "TableName": TABLE,
            "IndexName": VECTOR_INDEX,
            "SearchVector": [{"N": "1"}, {"N": "0"}],
            "TopK": 5,
        }),
    )
    .await;
    assert_eq!(
        searched["SearchResults"][0]["Item"]["pk"],
        json!({"S": "i1"}),
        "the vector index the statements could not read reports the item it holds, got \
         {searched}"
    );
}
