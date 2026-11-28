// SPDX-License-Identifier: CC0-1.0

//! What a paged read answers an `ExclusiveStartKey` that LEAVES OUT a key attribute of the
//! schema it is being read through.
//!
//! RULE: a resume token that does not carry every key attribute of the schema being paged
//! carries no key of that schema, so the read refuses it with `ValidationException` and the
//! one fixed invalid-token sentence — "The provided starting key is invalid" — with nothing
//! appended to it; a token that carries exactly those key attributes while stating one
//! of the TABLE's in another TYPE than the schema declares is no key of it either and earns
//! that same sentence. The sentence is fixed text: it names neither the missing
//! attribute nor the schema, and echoes nothing the request sent. An INDEX key attribute of
//! an index cursor stated in another type is the one element told further, by the read that
//! DETAILS it: the table walk names the index key it is, so it earns the typed
//! parameter-value sentence ([`index_key_type_mismatch`]), while a `Query` states the fixed
//! invalid-token sentence for that element too ([`Row::stated_by`]).
//!
//! The rows span what the rule generalises over: WHICH schema is being paged (a composite
//! base table, a global secondary index whose own key is composite, and a local one), WHICH
//! declared key attribute the token leaves out (the table's sort key, the index's sort key,
//! the partition key, every attribute at all), the read that carries the token (`Query` and
//! `Scan`), and the `Select` the read asks for (the projection the read defaults to and the
//! `COUNT` that projects nothing) — because the token is read through the schema whichever
//! read, whichever index and whichever projection reports it. A token that carries that
//! schema's key attributes with one TYPED against the schema is a row of the other half, and
//! a complete token is the control that pages instead of being refused.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The sentence a token that is no key of the schema being paged earns, written out here as
/// a literal rather than read off the crate under test, so the WORDING is what these rows
/// are held to.
const START_KEY_INVALID: &str = "The provided starting key is invalid";

/// The sentence a token that carries those key attributes with one of the TABLE's typed
/// against the schema earns: the same statement, since such a token is no key of that schema
/// either.
const START_KEY_TYPE_MISMATCH: &str = START_KEY_INVALID;

/// How that typed sentence opens, which is what tells a row the table walk DETAILS from a
/// row both reads state alike.
const TYPED_PREFIX: &str = "One or more parameter values were invalid: Type mismatch for Index \
                           Key";

/// The sentence an INDEX key element of an index cursor stated in another type than the
/// table's `AttributeDefinitions` declare for it earns, written out here as a literal for the
/// same reason as the one above.
fn index_key_type_mismatch(attribute: &str, expected: &str, actual: &str, index: &str) -> String {
    format!(
        "One or more parameter values were invalid: Type mismatch for Index Key {attribute} \
         Expected: {expected} Actual: {actual} IndexName: {index}"
    )
}

/// A table whose primary key is composite (`pk` HASH, `sk` RANGE), carrying a global
/// secondary index of its own composite key and a local one.
const TABLE: &str = "StartKeyMissingElement";

/// The global secondary index: `tenant` HASH, `rank` RANGE.
const GLOBAL_INDEX: &str = "ByTenantRank";

/// The local secondary index: the table's `pk` HASH, `rank` RANGE.
const LOCAL_INDEX: &str = "ByRank";

/// Create the table and seed one item, so a resumed page would have somewhere to resume
/// from and the refusals below are about the token rather than about an empty table.
async fn seed(router: &MultiServiceRouter) {
    ok(
        router,
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
    describe_when_active(router, "the indexed table", TABLE).await;
    ok(
        router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"pk": {"S": "one"}, "sk": {"N": "1"},
                     "tenant": {"S": "OPEN"}, "rank": {"N": "2"}},
        }),
    )
    .await;
}

/// One row: the schema a read pages, the token it carries, and the sentence that token
/// earns.
struct Row {
    case: &'static str,
    /// The index the read pages, where it pages one.
    index: Option<&'static str>,
    /// The partition a `Query`'s key condition selects, named for the schema being paged.
    partition: (&'static str, Value),
    token: Value,
    /// The sentence the table WALK states for this token.
    stated: String,
}

impl Row {
    /// The sentence `operation` states for this token: the table walk's own, except that a
    /// `Query` states the fixed invalid-token sentence for the one element that walk details
    /// (the module doc states the rule).
    fn stated_by(&self, operation: &str) -> &str {
        if operation == "Query" && self.stated.starts_with(TYPED_PREFIX) {
            START_KEY_INVALID
        } else {
            &self.stated
        }
    }
}

fn rows() -> Vec<Row> {
    let table_partition = ("pk", json!({"S": "one"}));
    let index_partition = ("tenant", json!({"S": "OPEN"}));
    vec![
        // The COMPOSITE base table: its token is its own primary key.
        Row {
            case: "the base table, the sort key left out",
            index: None,
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "the base table, the partition key left out",
            index: None,
            partition: table_partition.clone(),
            token: json!({"sk": {"N": "1"}}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "the base table, no attribute at all",
            index: None,
            partition: table_partition.clone(),
            token: json!({}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "the base table, the sort key typed against the schema",
            index: None,
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "sk": {"S": "1"}}),
            stated: START_KEY_TYPE_MISMATCH.to_string(),
        },
        // The GLOBAL secondary index: its cursor carries the index's key AND the table's.
        Row {
            case: "a global index, the index sort key left out",
            index: Some(GLOBAL_INDEX),
            partition: index_partition.clone(),
            token: json!({"tenant": {"S": "OPEN"}, "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "a global index, the table sort key left out",
            index: Some(GLOBAL_INDEX),
            partition: index_partition.clone(),
            token: json!({"tenant": {"S": "OPEN"}, "rank": {"N": "2"}, "pk": {"S": "one"}}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "a global index, only the index key",
            index: Some(GLOBAL_INDEX),
            partition: index_partition.clone(),
            token: json!({"tenant": {"S": "OPEN"}, "rank": {"N": "2"}}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "a global index, the index sort key typed against the schema",
            index: Some(GLOBAL_INDEX),
            partition: index_partition,
            token: json!({"tenant": {"S": "OPEN"}, "rank": {"S": "2"},
                          "pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: index_key_type_mismatch("rank", "N", "S", GLOBAL_INDEX),
        },
        // The LOCAL secondary index, whose partition key is the table's own.
        Row {
            case: "a local index, the table sort key left out",
            index: Some(LOCAL_INDEX),
            partition: table_partition.clone(),
            token: json!({"pk": {"S": "one"}, "rank": {"N": "2"}}),
            stated: START_KEY_INVALID.to_string(),
        },
        Row {
            case: "a local index, the index sort key left out",
            index: Some(LOCAL_INDEX),
            partition: table_partition,
            token: json!({"pk": {"S": "one"}, "sk": {"N": "1"}}),
            stated: START_KEY_INVALID.to_string(),
        },
    ]
}

/// The `Select` values a row is driven under: the projection each read defaults to, and the
/// `COUNT` that projects nothing at all — the token is read through the schema either way.
const SELECTS: [Option<&str>; 2] = [None, Some("COUNT")];

#[tokio::test]
async fn a_resume_token_short_of_a_declared_key_element_is_an_invalid_token() {
    let router = emulator();
    seed(&router).await;

    for row in rows() {
        for operation in ["Query", "Scan"] {
            for select in SELECTS {
                let case = format!(
                    "{} on {operation} with Select {}",
                    row.case,
                    select.unwrap_or("absent")
                );
                let mut request = json!({
                    "TableName": TABLE,
                    "ExclusiveStartKey": row.token.clone(),
                });
                if let Some(index) = row.index {
                    request["IndexName"] = json!(index);
                }
                if let Some(select) = select {
                    request["Select"] = json!(select);
                }
                if operation == "Query" {
                    let (attribute, value) = &row.partition;
                    request["KeyConditionExpression"] = json!(format!("{attribute} = :p"));
                    request["ExpressionAttributeValues"] = json!({":p": value.clone()});
                }
                let (status, body) = call(&router, operation, request).await;
                let answered = message(&body);
                let expected = row.stated_by(operation);
                assert_eq!(
                    (status, error_code(&body).as_str(), answered.as_str()),
                    (400, "ValidationException", expected),
                    "case {case:?}: expected the refusal {expected:?}, got {status} with {body}"
                );
                // The invalid-token sentence is fixed text, so nothing the token named
                // reaches it. The typed index-key sentence names its own key, type and
                // index, which the expectation above already holds it to.
                if expected == START_KEY_INVALID {
                    for named in ["pk", "sk", "tenant", "rank", GLOBAL_INDEX, LOCAL_INDEX] {
                        assert!(
                            !answered.contains(named),
                            "case {case:?}: the refusal must not name {named:?}, got \
                             {answered:?}"
                        );
                    }
                }
            }
        }
    }

    // The control: a token that carries every key attribute of the schema being paged pages
    // the read instead of earning either sentence, so the rows above are about the element
    // the token left out and not about the call.
    for (case, request) in [
        (
            "the base table's own key",
            json!({"TableName": TABLE, "Select": "COUNT",
                   "ExclusiveStartKey": {"pk": {"S": "one"}, "sk": {"N": "1"}}}),
        ),
        (
            "the global index's key beside the table's",
            json!({"TableName": TABLE, "IndexName": GLOBAL_INDEX, "Select": "COUNT",
                   "ExclusiveStartKey": {"tenant": {"S": "OPEN"}, "rank": {"N": "2"},
                                         "pk": {"S": "one"}, "sk": {"N": "1"}}}),
        ),
        (
            "the local index's key beside the table's",
            json!({"TableName": TABLE, "IndexName": LOCAL_INDEX, "Select": "COUNT",
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
