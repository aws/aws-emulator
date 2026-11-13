// SPDX-License-Identifier: CC0-1.0

//! WHICH sentence a paged read refuses an `ExclusiveStartKey` it cannot resume from with,
//! over the base table and over a secondary index.
//!
//! THE RULE: a resume token is read back through the schema being paged, and a token that is
//! no key of that schema AT ALL is refused as the invalid TOKEN it is — AWS's fixed sentence
//! naming neither the attribute nor the schema — whether it names an attribute the schema
//! declares no key for, carries one beside the key, or carries no attribute at all. A token
//! whose attributes ARE that schema's key attributes but which states one of them WRONGLY is
//! diagnosed further: that sentence states the token invalid and then the key-schema reason
//! after a colon. Two defects are stated that way on the BASE table — a key element carrying
//! another TYPE than the schema declares for it, and a declared key element the token left
//! out — while an INDEX cursor carries both schemas' key attributes, so one it does not
//! carry in full is not that cursor at all.
//!
//! The rows differ along HOW the token fails to be that key, along the SCHEMA being paged
//! (a single-attribute table, a composite one, a global secondary index and a local one),
//! and along the paged read that carries it (`Query` and `Scan`), because the token is read
//! through the schema whichever read and whichever index reports it.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// AWS's sentence for a token that is no key of the schema being paged.
const INVALID: &str = "The provided starting key is invalid";

/// AWS's sentence for a token whose attributes are that schema's key attributes but which
/// states one of them wrongly — carrying another type than declared, or leaving it out.
const TYPE_MISMATCH: &str =
    "The provided starting key is invalid: The provided key element does not match the schema";

/// A table whose primary key is ONE attribute.
const SOLO: &str = "ResumeTokenSolo";

/// A table whose primary key is composite, carrying a global secondary index whose own key
/// is composite and a local one.
const INDEXED: &str = "ResumeTokenIndexed";

/// The global secondary index of [`INDEXED`]: `tenant` HASH, `rank` RANGE.
const GLOBAL_INDEX: &str = "ByTenant";

/// The local secondary index of [`INDEXED`]: the table's `pk` HASH, `rank` RANGE.
const LOCAL_INDEX: &str = "ByRank";

/// Create both tables and seed one item in each, so a resumed page would have somewhere to
/// resume from.
async fn seed(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": SOLO,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": INDEXED,
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
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": GLOBAL_INDEX,
                "KeySchema": [
                    {"AttributeName": "tenant", "KeyType": "HASH"},
                    {"AttributeName": "rank", "KeyType": "RANGE"},
                ],
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
        }),
    )
    .await;
    describe_when_active(router, "the single-key table", SOLO).await;
    describe_when_active(router, "the indexed table", INDEXED).await;
    ok(
        router,
        "PutItem",
        json!({"TableName": SOLO, "Item": {"pk": {"S": "one"}}}),
    )
    .await;
    ok(
        router,
        "PutItem",
        json!({
            "TableName": INDEXED,
            "Item": {"pk": {"S": "one"}, "sk": {"N": "1"},
                     "tenant": {"S": "OPEN"}, "rank": {"N": "2"}},
        }),
    )
    .await;
}

/// One row: the schema being paged, the token, and the sentence that token earns.
struct Row {
    case: &'static str,
    table: &'static str,
    /// The index the read pages, where it pages one.
    index: Option<&'static str>,
    /// The partition value a `Query`'s key condition selects, named for the schema paged.
    partition: (&'static str, Value),
    token: Value,
    stated: &'static str,
}

fn rows() -> Vec<Row> {
    let open = ("tenant", json!({"S": "OPEN"}));
    let one = ("pk", json!({"S": "one"}));
    vec![
        // The base table of ONE key attribute.
        Row {
            case: "a single-key table, an attribute the schema declares no key for",
            table: SOLO,
            index: None,
            partition: one.clone(),
            token: json!({"nope": {"S": "x"}}),
            stated: INVALID,
        },
        Row {
            case: "a single-key table, a spare attribute beside the key",
            table: SOLO,
            index: None,
            partition: one.clone(),
            token: json!({"pk": {"S": "one"}, "spare": {"S": "x"}}),
            stated: INVALID,
        },
        Row {
            case: "a single-key table, no attribute at all",
            table: SOLO,
            index: None,
            partition: one.clone(),
            token: json!({}),
            stated: INVALID,
        },
        Row {
            case: "a single-key table, the key attribute as a number",
            table: SOLO,
            index: None,
            partition: one.clone(),
            token: json!({"pk": {"N": "1"}}),
            stated: TYPE_MISMATCH,
        },
        Row {
            case: "a single-key table, the key attribute as a string set",
            table: SOLO,
            index: None,
            partition: one.clone(),
            token: json!({"pk": {"SS": ["one"]}}),
            stated: TYPE_MISMATCH,
        },
        // The COMPOSITE base table.
        Row {
            case: "a composite table, the sort key left out",
            table: INDEXED,
            index: None,
            partition: one.clone(),
            token: json!({"pk": {"S": "one"}}),
            stated: TYPE_MISMATCH,
        },
        Row {
            case: "a composite table, a third attribute beside both keys",
            table: INDEXED,
            index: None,
            partition: one.clone(),
            token: json!({"pk": {"S": "one"}, "sk": {"N": "1"}, "spare": {"S": "x"}}),
            stated: INVALID,
        },
        Row {
            case: "a composite table, the sort key as a string",
            table: INDEXED,
            index: None,
            partition: one.clone(),
            token: json!({"pk": {"S": "one"}, "sk": {"S": "1"}}),
            stated: TYPE_MISMATCH,
        },
        // The GLOBAL secondary index: its token carries the index's key AND the table's.
        Row {
            case: "a global index, the index sort key left out",
            table: INDEXED,
            index: Some(GLOBAL_INDEX),
            partition: open.clone(),
            token: json!({"tenant": {"S": "OPEN"}, "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: INVALID,
        },
        Row {
            case: "a global index, an attribute neither schema declares",
            table: INDEXED,
            index: Some(GLOBAL_INDEX),
            partition: open.clone(),
            token: json!({"tenant": {"S": "OPEN"}, "rank": {"N": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}, "spare": {"S": "x"}}),
            stated: INVALID,
        },
        Row {
            case: "a global index, the index sort key as a string",
            table: INDEXED,
            index: Some(GLOBAL_INDEX),
            partition: open.clone(),
            token: json!({"tenant": {"S": "OPEN"}, "rank": {"S": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: TYPE_MISMATCH,
        },
        // The LOCAL secondary index, whose partition key is the table's own.
        Row {
            case: "a local index, the table sort key left out",
            table: INDEXED,
            index: Some(LOCAL_INDEX),
            partition: one.clone(),
            token: json!({"pk": {"S": "one"}, "rank": {"N": "2"}}),
            stated: INVALID,
        },
        Row {
            case: "a local index, the index sort key as a string",
            table: INDEXED,
            index: Some(LOCAL_INDEX),
            partition: one,
            token: json!({"pk": {"S": "one"}, "rank": {"S": "2"}, "sk": {"N": "1"}}),
            stated: TYPE_MISMATCH,
        },
    ]
}

#[tokio::test]
async fn a_resume_token_that_is_no_key_of_the_schema_states_its_own_sentence() {
    let router = emulator();
    seed(&router).await;

    for row in rows() {
        for operation in ["Query", "Scan"] {
            let case = format!("{} on {operation}", row.case);
            let mut request = json!({
                "TableName": row.table,
                "ExclusiveStartKey": row.token.clone(),
            });
            if let Some(index) = row.index {
                request["IndexName"] = json!(index);
            }
            if operation == "Query" {
                let (attribute, value) = &row.partition;
                request["KeyConditionExpression"] = json!(format!("{attribute} = :p"));
                request["ExpressionAttributeValues"] = json!({":p": value.clone()});
            }
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, "ValidationException", row.stated),
                "case {case:?}: expected the refusal {:?}, got {status} with {body}",
                row.stated
            );
        }
    }

    // The control: a token that IS a key of the schema being paged pages the read rather
    // than earning either sentence, so the refusals above are about the token's shape.
    for (case, request) in [
        (
            "the base table's own key",
            json!({"TableName": INDEXED,
                   "ExclusiveStartKey": {"pk": {"S": "one"}, "sk": {"N": "1"}}}),
        ),
        (
            "the global index's key beside the table's",
            json!({"TableName": INDEXED, "IndexName": GLOBAL_INDEX,
                   "ExclusiveStartKey": {"tenant": {"S": "OPEN"}, "rank": {"N": "2"},
                                         "pk": {"S": "one"}, "sk": {"N": "1"}}}),
        ),
        (
            "the local index's key beside the table's",
            json!({"TableName": INDEXED, "IndexName": LOCAL_INDEX,
                   "ExclusiveStartKey": {"pk": {"S": "one"}, "rank": {"N": "2"},
                                         "sk": {"N": "1"}}}),
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
