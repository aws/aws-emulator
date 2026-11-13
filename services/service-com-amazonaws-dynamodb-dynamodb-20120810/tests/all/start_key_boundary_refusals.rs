// SPDX-License-Identifier: CC0-1.0

//! How the paged reads refuse an `ExclusiveStartKey` they cannot resume from, and WHICH
//! of the two refusals each case gets.
//!
//! The rules pinned here:
//!
//! 1. a token that is no key of the schema being paged AT ALL is refused as the invalid TOKEN
//!    it is — the sentence states the starting key invalid, and names neither the attribute
//!    nor the schema — whichever paged read carries it, while one whose attributes ARE that
//!    schema's key attributes but which states one of them wrongly (another type than
//!    declared, or left out) states the key-schema reason after a colon;
//! 2. a `Query` token that IS a key of that schema but names a key the
//!    `KeyConditionExpression` does not select is refused by the HALF of the condition it
//!    falls outside, each with its own fixed sentence: a token in another partition is
//!    outside the boundaries the condition draws, and one whose sort key the condition's
//!    comparison excludes does not match the range key predicate;
//! 3. a token the condition DOES select resumes the page strictly after it, in the
//!    direction the query walks, so the two refusals never stand in the way of paging.
//!
//! Rule 2 is asserted over a TABLE of conditions spanning the comparisons a key condition
//! has — equality on the partition key alone, `>`, `<`, `BETWEEN`, `=` and `begins_with`
//! on the sort key — in both scan directions, because the boundaries are the condition's
//! whatever shape it takes. Rule 1 is asserted over a table of malformed tokens crossed
//! with both reads.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// AWS's fixed sentence for a starting key whose attributes are the key attributes of the
/// schema being paged but which states one of them wrongly — carrying another type than the
/// schema declares for it, or leaving it out: the invalid-token statement and the key-schema
/// reason after the colon.
const START_KEY_TYPE_MISMATCH: &str =
    "The provided starting key is invalid: The provided key element does not match the schema";

/// AWS's fixed sentence for a starting key that is no key of the schema being paged at all:
/// the invalid-token statement alone.
const START_KEY_INVALID: &str = "The provided starting key is invalid";

/// AWS's fixed sentence for a starting key that is a key of the schema but lies outside
/// the PARTITION the query's condition selects.
const START_KEY_OUTSIDE_BOUNDARIES: &str =
    "The provided starting key is outside query boundaries based on provided conditions";

/// AWS's fixed sentence for a starting key inside that partition whose SORT key value the
/// condition's comparison excludes: the comparison is a predicate the resume point has to
/// satisfy, and the refusal names that predicate.
const START_KEY_OUTSIDE_PREDICATE: &str =
    "The provided starting key does not match the range key predicate";

/// A table whose sort key is a NUMBER, holding two partitions of five items each.
const NUMERIC_TABLE: &str = "StartKeyBoundaries";

/// A table whose sort key is a STRING, which is the only kind `begins_with` means
/// anything for.
const PREFIX_TABLE: &str = "StartKeyPrefixes";

/// Create both tables and seed their items.
async fn seed(router: &MultiServiceRouter) {
    for (table, sort_type) in [(NUMERIC_TABLE, "N"), (PREFIX_TABLE, "S")] {
        ok(
            router,
            "CreateTable",
            json!({
                "TableName": table,
                "KeySchema": [
                    {"AttributeName": "pk", "KeyType": "HASH"},
                    {"AttributeName": "sk", "KeyType": "RANGE"},
                ],
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": "sk", "AttributeType": sort_type},
                ],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    }
    describe_when_active(router, "the numeric-sort table", NUMERIC_TABLE).await;
    describe_when_active(router, "the string-sort table", PREFIX_TABLE).await;

    for partition in ["one", "two"] {
        for row in 0..5 {
            ok(
                router,
                "PutItem",
                json!({
                    "TableName": NUMERIC_TABLE,
                    "Item": {"pk": {"S": partition}, "sk": {"N": row.to_string()}},
                }),
            )
            .await;
        }
    }
    for sort in ["a0", "a1", "b0"] {
        ok(
            router,
            "PutItem",
            json!({
                "TableName": PREFIX_TABLE,
                "Item": {"pk": {"S": "one"}, "sk": {"S": sort}},
            }),
        )
        .await;
    }
}

/// One `Query` whose `ExclusiveStartKey` is a key of the table's schema: the condition it
/// states, the direction it walks and the token it resumes from.
struct Resume {
    case: &'static str,
    table: &'static str,
    condition: &'static str,
    values: Value,
    forward: bool,
    start: Value,
}

impl Resume {
    /// The request this row sends.
    fn request(&self) -> Value {
        json!({
            "TableName": self.table,
            "KeyConditionExpression": self.condition,
            "ExpressionAttributeValues": self.values.clone(),
            "ScanIndexForward": self.forward,
            "ExclusiveStartKey": self.start.clone(),
        })
    }
}

/// The sort key of every item a page reports, in the order it reported them.
fn sort_keys(page: &Value) -> Vec<String> {
    page.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item["sk"]["N"]
                        .as_str()
                        .or_else(|| item["sk"]["S"].as_str())
                        .unwrap_or("<no sk>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_starting_key_outside_the_key_conditions_is_refused_as_out_of_boundaries() {
    let router = emulator();
    seed(&router).await;

    // Each row: a token that IS a key of the table's schema but names a key the
    // condition does not select — another partition, or a sort key each comparison the
    // member has excludes, in both directions — paired with the half of the condition it
    // falls outside, which is what decides the sentence.
    let outside: Vec<(Resume, &'static str)> = vec![
        (
            Resume {
                case: "a token from another partition than the condition's",
                table: NUMERIC_TABLE,
                condition: "pk = :p",
                values: json!({":p": {"S": "one"}}),
                forward: true,
                start: json!({"pk": {"S": "two"}, "sk": {"N": "1"}}),
            },
            START_KEY_OUTSIDE_BOUNDARIES,
        ),
        (
            Resume {
                case: "a token below a greater-than sort condition",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk > :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "2"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "1"}}),
            },
            START_KEY_OUTSIDE_PREDICATE,
        ),
        (
            Resume {
                case: "a token above a less-than sort condition",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk < :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "2"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "3"}}),
            },
            START_KEY_OUTSIDE_PREDICATE,
        ),
        (
            Resume {
                case: "a token past the top of a BETWEEN sort condition",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk BETWEEN :lo AND :hi",
                values: json!({":p": {"S": "one"}, ":lo": {"N": "1"}, ":hi": {"N": "3"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "4"}}),
            },
            START_KEY_OUTSIDE_PREDICATE,
        ),
        (
            Resume {
                case: "a token other than the one sort key an equality condition selects",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk = :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "2"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "3"}}),
            },
            START_KEY_OUTSIDE_PREDICATE,
        ),
        (
            Resume {
                case: "a token outside a greater-than condition read descending",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk > :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "2"}}),
                forward: false,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "1"}}),
            },
            START_KEY_OUTSIDE_PREDICATE,
        ),
        (
            Resume {
                case: "a token outside a begins_with sort condition",
                table: PREFIX_TABLE,
                condition: "pk = :p AND begins_with(sk, :prefix)",
                values: json!({":p": {"S": "one"}, ":prefix": {"S": "a"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"S": "b0"}}),
            },
            START_KEY_OUTSIDE_PREDICATE,
        ),
        (
            // A token in another partition whose sort key the comparison ALSO excludes:
            // the partition is the boundary of what the query reads at all, so that is
            // the half it is refused by.
            Resume {
                case: "a token from another partition and below the sort condition",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk > :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "2"}}),
                forward: true,
                start: json!({"pk": {"S": "two"}, "sk": {"N": "1"}}),
            },
            START_KEY_OUTSIDE_BOUNDARIES,
        ),
    ];

    for (row, stated) in &outside {
        let (status, body) = call(&router, "Query", row.request()).await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ValidationException", *stated),
            "case {:?}: expected the refusal {stated:?}, got {status} with {body}",
            row.case
        );
    }

    // The refusals page nothing: a token the condition DOES select still resumes the walk
    // strictly after it, in the direction the query reads.
    let inside: Vec<(Resume, Vec<&str>)> = vec![
        (
            Resume {
                case: "a token inside a greater-than sort condition",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk > :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "1"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "2"}}),
            },
            vec!["3", "4"],
        ),
        (
            Resume {
                case: "a token at the top of a BETWEEN sort condition",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk BETWEEN :lo AND :hi",
                values: json!({":p": {"S": "one"}, ":lo": {"N": "1"}, ":hi": {"N": "3"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "3"}}),
            },
            vec![],
        ),
        (
            Resume {
                case: "a token inside a condition read descending",
                table: NUMERIC_TABLE,
                condition: "pk = :p AND sk < :s",
                values: json!({":p": {"S": "one"}, ":s": {"N": "4"}}),
                forward: false,
                start: json!({"pk": {"S": "one"}, "sk": {"N": "2"}}),
            },
            vec!["1", "0"],
        ),
        (
            Resume {
                case: "a token inside a begins_with sort condition",
                table: PREFIX_TABLE,
                condition: "pk = :p AND begins_with(sk, :prefix)",
                values: json!({":p": {"S": "one"}, ":prefix": {"S": "a"}}),
                forward: true,
                start: json!({"pk": {"S": "one"}, "sk": {"S": "a0"}}),
            },
            vec!["a1"],
        ),
    ];

    for (row, expected) in &inside {
        let page = ok(&router, "Query", row.request()).await;
        let expected: Vec<String> = expected.iter().map(|sort| (*sort).to_string()).collect();
        assert_eq!(
            sort_keys(&page),
            expected,
            "case {:?}: the page resumes strictly after the token, got {page}",
            row.case
        );
        assert_eq!(
            page["Count"].clone(),
            json!(expected.len()),
            "case {:?}: Count is what the resumed page answered, got {page}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_starting_key_that_is_no_key_of_the_schema_states_the_schema_reason() {
    let router = emulator();
    seed(&router).await;

    // Each row: HOW the token fails to be a key of the table's schema, and the sentence
    // that failure earns — the schema reason for a token whose attributes are this schema's
    // key attributes but which states one of them wrongly (another type than declared, or
    // left out), and the bare invalid-token sentence for a map that is no key of the schema
    // at all.
    let tokens: Vec<(&str, Value, &str)> = vec![
        (
            "an attribute the schema does not declare",
            json!({"nope": {"S": "x"}}),
            START_KEY_INVALID,
        ),
        (
            "only the partition key of a composite schema",
            json!({"pk": {"S": "one"}}),
            START_KEY_TYPE_MISMATCH,
        ),
        (
            "the sort key with the type the schema does not declare",
            json!({"pk": {"S": "one"}, "sk": {"S": "1"}}),
            START_KEY_TYPE_MISMATCH,
        ),
        (
            "the partition key with the type the schema does not declare",
            json!({"pk": {"N": "1"}, "sk": {"N": "1"}}),
            START_KEY_TYPE_MISMATCH,
        ),
        (
            "a third attribute beside both keys",
            json!({"pk": {"S": "one"}, "sk": {"N": "1"}, "spare": {"S": "x"}}),
            START_KEY_INVALID,
        ),
        ("no attribute at all", json!({}), START_KEY_INVALID),
    ];

    for (case, token, stated) in &tokens {
        for operation in ["Query", "Scan"] {
            let mut request = json!({
                "TableName": NUMERIC_TABLE,
                "ExclusiveStartKey": token.clone(),
            });
            if operation == "Query" {
                request["KeyConditionExpression"] = json!("pk = :p");
                request["ExpressionAttributeValues"] = json!({":p": {"S": "one"}});
            }
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                (status, error_code(&body).as_str(), message(&body).as_str()),
                (400, "ValidationException", *stated),
                "case {case:?} on {operation}: expected the refusal {stated:?}, got {status} \
                 with {body}"
            );
        }
    }
}
