// SPDX-License-Identifier: CC0-1.0

//! Which ENVELOPE a refusal of an enumerated member carries, and which value set it states.
//!
//! Two rules are pinned here, each over a TABLE spanning what it generalises over rather
//! than one recorded request:
//!
//! * a value outside an enumerated member's set is refused `ValidationException` with the
//!   clause naming the member path and the rejected value, and the clause is stated BARE —
//!   outside the `<N> validation error(s) detected: ` counting envelope — for the members an
//!   operation reads inside a declaration it was handed (the comparator of a legacy
//!   condition entry, the projection type of an index declaration, a vector index's distance
//!   function), while a member of the input ITSELF is counted in that envelope;
//! * the admissible set the clause ends with is stated in the order THIS SERVICE states it
//!   in, which is a property of the enum and not of its declaration order.
//!
//! The rows vary the operation, the member, how deep in the request it sits and the FORM of
//! the rejected value (a misspelling, a declared value in the wrong case, a value belonging
//! to another enum). Everything is driven over the wire, so nothing is asserted that a
//! client could not observe.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every row addresses, created and active before any row runs so no row is
/// answered by a missing table instead of by the constraint.
const TABLE: &str = "EnumEnvelopes";

/// The envelope a refusal of a member of the input itself carries.
const ENVELOPE: &str = "1 validation error detected: ";

/// One row: the case, the operation, the request, and the clause the refusal must carry
/// whole — envelope included where the member is one the envelope counts.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: fn() -> Value,
    expected: &'static str,
}

/// A `CreateTable` body for a table keyed on `pk` alone, with `extra` merged in.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "sk", "AttributeType": "S"},
            {"AttributeName": "vec", "AttributeType": "B"},
        ],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The rows: the three members stated BARE, each reached through a different carrier, and
/// the members of an input itself that are counted in the envelope beside them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the comparator of a legacy key condition, misspelled",
            operation: "Query",
            request: || {
                json!({"TableName": TABLE,
                       "KeyConditions": {"pk": {"ComparisonOperator": "EQUALS",
                                                "AttributeValueList": [{"S": "a"}]}}})
            },
            expected: "Value 'EQUALS' at 'keyConditions.pk.member.comparisonOperator' failed \
                       to satisfy constraint: Member must satisfy enum value set: [IN, NULL, \
                       BETWEEN, LT, NOT_CONTAINS, EQ, GT, NOT_NULL, NE, LE, BEGINS_WITH, GE, \
                       CONTAINS]",
        },
        Row {
            case: "the comparator of a legacy scan filter, in the wrong case",
            operation: "Scan",
            request: || {
                json!({"TableName": TABLE,
                       "ScanFilter": {"tag": {"ComparisonOperator": "eq",
                                              "AttributeValueList": [{"S": "a"}]}}})
            },
            expected: "Value 'eq' at 'scanFilter.tag.member.comparisonOperator' failed to \
                       satisfy constraint: Member must satisfy enum value set: [IN, NULL, \
                       BETWEEN, LT, NOT_CONTAINS, EQ, GT, NOT_NULL, NE, LE, BEGINS_WITH, GE, \
                       CONTAINS]",
        },
        Row {
            case: "the projection type of a LOCAL index declaration",
            operation: "CreateTable",
            request: || {
                create_body(
                    "LsiProjection",
                    json!({
                        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                                      {"AttributeName": "sk", "KeyType": "RANGE"}],
                        "LocalSecondaryIndexes": [{
                            "IndexName": "by-sk",
                            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                                          {"AttributeName": "sk", "KeyType": "RANGE"}],
                            "Projection": {"ProjectionType": "KEYS"},
                        }],
                    }),
                )
            },
            expected: "Value 'KEYS' at \
                       'localSecondaryIndexes.1.member.projection.projectionType' failed to \
                       satisfy constraint: Member must satisfy enum value set: [ALL, \
                       KEYS_ONLY, INCLUDE]",
        },
        Row {
            case: "the projection type of a GLOBAL index declaration",
            operation: "CreateTable",
            request: || {
                create_body(
                    "GsiProjection",
                    json!({
                        "GlobalSecondaryIndexes": [{
                            "IndexName": "by-sk",
                            "KeySchema": [{"AttributeName": "sk", "KeyType": "HASH"}],
                            "Projection": {"ProjectionType": "EVERYTHING"},
                        }],
                    }),
                )
            },
            expected: "Value 'EVERYTHING' at \
                       'globalSecondaryIndexes.1.member.projection.projectionType' failed to \
                       satisfy constraint: Member must satisfy enum value set: [ALL, \
                       KEYS_ONLY, INCLUDE]",
        },
        Row {
            case: "the distance function of a vector index declaration",
            operation: "CreateTable",
            request: || {
                create_body(
                    "VectorDistance",
                    json!({
                        "VectorIndexes": [{
                            "IndexName": "by-vec",
                            "VectorAttribute": {"AttributeName": "vec"},
                            "Dimensions": 4,
                            "DistanceFunction": "MANHATTAN",
                            "Projection": {"ProjectionType": "ALL"},
                        }],
                    }),
                )
            },
            expected: "Value 'MANHATTAN' at 'vectorIndexes.1.member.distanceFunction' failed \
                       to satisfy constraint: Member must satisfy enum value set: \
                       [DOT_PRODUCT, COSINE, EUCLIDEAN]",
        },
    ]
}

/// The rows whose member is one of the input's OWN — the boundary of the rule above, so the
/// envelope is visible as a difference rather than asserted nowhere.
fn enveloped_rows() -> Vec<Row> {
    vec![
        Row {
            case: "a legacy per-attribute update's action",
            operation: "UpdateItem",
            request: || {
                json!({"TableName": TABLE, "Key": {"pk": {"S": "one"}},
                       "AttributeUpdates": {"v": {"Action": "REPLACE", "Value": {"S": "x"}}}})
            },
            expected: "Value 'REPLACE' at 'attributeUpdates.v.member.action' failed to \
                       satisfy constraint: Member must satisfy enum value set: [ADD, DELETE, \
                       PUT]",
        },
        Row {
            case: "a backup listing's type filter, in the wrong case",
            operation: "ListBackups",
            request: || json!({"BackupType": "user"}),
            expected: "Value 'user' at 'backupType' failed to satisfy constraint: Member \
                       must satisfy enum value set: [ALL, AWS_BACKUP, USER, SYSTEM]",
        },
        Row {
            case: "a table walk's own Select",
            operation: "Scan",
            request: || json!({"TableName": TABLE, "Select": "EVERYTHING"}),
            expected: "Value 'EVERYTHING' at 'select' failed to satisfy constraint: Member \
                       must satisfy enum value set: [SPECIFIC_ATTRIBUTES, COUNT, \
                       ALL_ATTRIBUTES, ALL_PROJECTED_ATTRIBUTES]",
        },
    ]
}

/// Call `operation`, require 400 `ValidationException`, and answer the message.
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
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
         {body}"
    );
    message(&body)
}

#[tokio::test]
async fn the_members_read_inside_a_declaration_state_their_clause_bare() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;

    for row in rows() {
        let stated = refusal(&router, row.case, row.operation, (row.request)()).await;
        assert_eq!(
            stated, row.expected,
            "case {case:?}: expected the clause alone, got {stated:?}",
            case = row.case
        );
    }
}

#[tokio::test]
async fn a_member_of_the_input_itself_is_counted_in_the_envelope() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;

    for row in enveloped_rows() {
        let stated = refusal(&router, row.case, row.operation, (row.request)()).await;
        assert_eq!(
            stated,
            format!("{ENVELOPE}{}", row.expected),
            "case {case:?}: expected the clause inside the counting envelope, got {stated:?}",
            case = row.case
        );
    }
}

#[tokio::test]
async fn a_refused_declaration_creates_nothing_and_the_corrected_one_is_served() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;

    // The refused create left no table behind, so the refusals above are about the member
    // and nothing else.
    let (status, listed) = call(&router, "ListTables", json!({})).await;
    assert_eq!(status, 200, "ListTables answered {status} with {listed}");
    let names: Vec<&str> = listed["TableNames"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![TABLE],
        "case a refused declaration: only the seeded table exists, got {names:?}"
    );

    // And the same declaration with a value the set names is served: the index is declared
    // and the description reports it, so the refusal is the member's and not the shape's.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "GsiProjectionFixed",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                     {"AttributeName": "sk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": "by-sk",
                "KeySchema": [{"AttributeName": "sk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
            }],
        }),
    )
    .await;
    let described = crate::emulator::describe_when_active(
        &router,
        "the corrected declaration",
        "GsiProjectionFixed",
    )
    .await;
    assert_eq!(
        described["GlobalSecondaryIndexes"][0]["Projection"]["ProjectionType"],
        json!("KEYS_ONLY"),
        "case the corrected declaration: the index is declared as asked, got {described}"
    );
}
