// SPDX-License-Identifier: CC0-1.0

//! The rules a filter is evaluated by, the rules a request's expression members are
//! validated by, and the ceilings a write is admitted from.
//!
//! Five rules are under test, each over a TABLE of inputs rather than one request:
//!
//! 1. a NOT-EQUAL comparison needs no attribute: an item that carries the path no value
//!    at all is not equal to any value, so it satisfies `<>` (and the legacy `NE` that
//!    lowers onto it), while every comparison that needs a value to compare — equality
//!    and the orderings — is not satisfied by an absent attribute;
//! 2. a function name the expression languages do not have is refused with the
//!    member-qualified template `Invalid <Member>: Invalid function name; function:
//!    <name>`, whichever expression member carried it;
//! 3. every key of `ExpressionAttributeNames`/`ExpressionAttributeValues` must be
//!    referenced by one of the request's expressions, and the request is refused naming
//!    the member and the unreferenced keys when one is not;
//! 4. a PartiQL `SELECT` is charged for what its PLAN read: a `WHERE` clause that pins
//!    the partition key by equality is planned as a query of that partition, one that
//!    pins none is planned as a full table scan, and a predicate on a non-key attribute
//!    never reduces what the plan read;
//! 5. a write is admitted from the capacity its table is provisioned for AND from the
//!    per-partition-key ceiling beneath it, a write neither covers is throttled with the
//!    `ThrottlingReasons` entry of the ceiling that bound, and a throttled write leaves
//!    nothing stored.
//!
//! The rows differ along what each rule generalises over: the comparator and the shape
//! of the value compared, the expression member and the operation carrying it, which
//! substitution table is unused and how many of its keys, the form of the statement's
//! `WHERE` clause, and whether a write stream is aimed at one partition key or spread
//! over many.

use serde_json::{json, Map, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok, REGION};

/// The ARN form every refusal here reports a table by.
fn arn_of(table: &str) -> String {
    format!("arn:aws:dynamodb:{REGION}:000000000000:table/{table}")
}

/// Create a table with the given key schema and billing members, and wait out its
/// `CREATING` span.
async fn table_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    keys: &[(&str, &str, &str)],
    billing: Value,
) {
    let key_schema: Vec<Value> = keys
        .iter()
        .map(|(attribute, key_type, _)| json!({"AttributeName": attribute, "KeyType": key_type}))
        .collect();
    let definitions: Vec<Value> = keys
        .iter()
        .map(|(attribute, _, attribute_type)| {
            json!({"AttributeName": attribute, "AttributeType": attribute_type})
        })
        .collect();
    let mut body = json!({
        "TableName": name,
        "KeySchema": key_schema,
        "AttributeDefinitions": definitions,
    });
    for (member, value) in billing.as_object().expect("the billing members are a map") {
        body[member.clone()] = value.clone();
    }
    ok(router, "CreateTable", body).await;
    describe_when_active(router, name, name).await;
}

/// The `id` of every item a read reported, in the order it reported them.
fn reported_ids(answered: &Value) -> Vec<String> {
    answered["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["id"]["S"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The same, sorted, for an assertion about the SET of items a filter selected.
fn selected_ids(answered: &Value) -> Vec<String> {
    let mut ids = reported_ids(answered);
    ids.sort();
    ids
}

// ===========================================================================
// 1. A not-equal comparison needs no attribute
// ===========================================================================

/// The items every filter row below is evaluated against: one carrying the compared
/// attribute as a string, one carrying it as a number, one carrying it as the empty
/// string, one carrying it NULL-typed, and one not carrying it at all.
fn sparse_items() -> Vec<Value> {
    vec![
        json!({"id": {"S": "string"}, "tag": {"S": "active"}}),
        json!({"id": {"S": "number"}, "tag": {"N": "7"}}),
        json!({"id": {"S": "empty"}, "tag": {"S": ""}}),
        json!({"id": {"S": "null-typed"}, "tag": {"NULL": true}}),
        json!({"id": {"S": "absent"}, "other": {"N": "5"}}),
    ]
}

/// One filter row: the case, the expression, its values, and the ids it must select.
struct FilterRow {
    case: &'static str,
    expression: &'static str,
    values: Value,
    selected: &'static [&'static str],
}

/// The rows differ in the COMPARATOR and in the shape of the value compared: the
/// not-equal comparator against an empty string, a non-empty string and a number; the
/// equality that needs the attribute present; an ordering comparator, which needs two
/// values to order; the same not-equal comparison written as the complement of an
/// equality with `NOT`; and the conjunction with `attribute_exists`, which is the only
/// way to ask for "present AND not equal".
fn filter_rows() -> Vec<FilterRow> {
    vec![
        FilterRow {
            case: "not-equal against the empty string",
            expression: "tag <> :v",
            values: json!({":v": {"S": ""}}),
            selected: &["absent", "null-typed", "number", "string"],
        },
        FilterRow {
            case: "not-equal against a non-empty string one item holds",
            expression: "tag <> :v",
            values: json!({":v": {"S": "active"}}),
            selected: &["absent", "empty", "null-typed", "number"],
        },
        FilterRow {
            case: "not-equal against a number one item holds",
            expression: "tag <> :v",
            values: json!({":v": {"N": "7"}}),
            selected: &["absent", "empty", "null-typed", "string"],
        },
        FilterRow {
            case: "not-equal against a value no item holds",
            expression: "tag <> :v",
            values: json!({":v": {"S": "nothing-holds-this"}}),
            selected: &["absent", "empty", "null-typed", "number", "string"],
        },
        FilterRow {
            case: "equality, which needs the attribute present and equal",
            expression: "tag = :v",
            values: json!({":v": {"S": ""}}),
            selected: &["empty"],
        },
        FilterRow {
            case: "an ordering comparator, which needs two values to order",
            expression: "tag > :v",
            values: json!({":v": {"S": ""}}),
            selected: &["string"],
        },
        FilterRow {
            case: "the complement of an equality, written with NOT",
            expression: "NOT tag = :v",
            values: json!({":v": {"S": ""}}),
            selected: &["absent", "null-typed", "number", "string"],
        },
        FilterRow {
            case: "present AND not equal, which is what the conjunction asks for",
            expression: "attribute_exists(tag) AND tag <> :v",
            values: json!({":v": {"S": ""}}),
            selected: &["null-typed", "number", "string"],
        },
    ]
}

#[tokio::test]
async fn a_not_equal_comparison_selects_items_that_do_not_carry_the_attribute() {
    let router = emulator();
    table_with(
        &router,
        "Sparse",
        &[("id", "HASH", "S")],
        json!({"BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    let items = sparse_items();
    for item in &items {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Sparse", "Item": item}),
        )
        .await;
    }

    for row in filter_rows() {
        let answered = ok(
            &router,
            "Scan",
            json!({"TableName": "Sparse", "ConsistentRead": true,
                   "FilterExpression": row.expression,
                   "ExpressionAttributeValues": row.values}),
        )
        .await;
        let expected: Vec<String> = row.selected.iter().map(ToString::to_string).collect();
        assert_eq!(
            selected_ids(&answered),
            expected,
            "case {:?}: expected the filter to select {expected:?}, got {answered}",
            row.case
        );
        assert_eq!(
            answered["Count"],
            json!(row.selected.len()),
            "case {:?}: Count counts what the filter kept, got {answered}",
            row.case
        );
        assert_eq!(
            answered["ScannedCount"],
            json!(items.len()),
            "case {:?}: a filter is applied after the page is read, so every item is \
             still evaluated, got {answered}",
            row.case
        );
    }

    // The invariant behind the rows: for any value, `=` and `<>` PARTITION the table —
    // every item satisfies exactly one of them — which is what makes `<>` usable as the
    // complement of `=`.
    for value in [
        json!({"S": ""}),
        json!({"S": "active"}),
        json!({"N": "7"}),
        json!({"NULL": true}),
        json!({"S": "unheld"}),
    ] {
        let scan = |expression: &'static str| {
            let value = value.clone();
            let router = &router;
            async move {
                ok(
                    router,
                    "Scan",
                    json!({"TableName": "Sparse", "ConsistentRead": true,
                           "FilterExpression": expression,
                           "ExpressionAttributeValues": {":v": value}}),
                )
                .await
            }
        };
        let equal = selected_ids(&scan("tag = :v").await);
        let unequal = selected_ids(&scan("tag <> :v").await);
        let mut together: Vec<String> = equal.iter().chain(unequal.iter()).cloned().collect();
        together.sort();
        together.dedup();
        let mut every: Vec<String> = sparse_items()
            .iter()
            .map(|item| item["id"]["S"].as_str().unwrap_or_default().to_string())
            .collect();
        every.sort();
        assert_eq!(
            together, every,
            "case value {value}: = and <> must cover every item, got {equal:?} and {unequal:?}"
        );
        assert_eq!(
            equal.len() + unequal.len(),
            every.len(),
            "case value {value}: = and <> must not both hold for one item, got {equal:?} \
             and {unequal:?}"
        );
    }

    // The same rule on the other surfaces the comparison has: the legacy `NE` operator
    // the expression replaced, and a Query's own `FilterExpression`.
    let legacy = ok(
        &router,
        "Scan",
        json!({"TableName": "Sparse", "ConsistentRead": true,
               "ScanFilter": {"tag": {"ComparisonOperator": "NE",
                                      "AttributeValueList": [{"S": ""}]}}}),
    )
    .await;
    assert_eq!(
        selected_ids(&legacy),
        vec!["absent", "null-typed", "number", "string"],
        "case the legacy NE operator: it selects what <> selects, got {legacy}"
    );
    let queried = ok(
        &router,
        "Query",
        json!({"TableName": "Sparse", "ConsistentRead": true,
               "KeyConditionExpression": "id = :id",
               "FilterExpression": "tag <> :v",
               "ExpressionAttributeValues": {":id": {"S": "absent"}, ":v": {"S": ""}}}),
    )
    .await;
    assert_eq!(
        selected_ids(&queried),
        vec!["absent"],
        "case a Query filter: the item carrying no tag satisfies <> there too, got {queried}"
    );

    // And on the WRITE path: a condition stated with <> over an attribute the stored
    // item does not carry holds, so the write runs — read back through GetItem.
    ok(
        &router,
        "UpdateItem",
        json!({"TableName": "Sparse", "Key": {"id": {"S": "absent"}},
               "UpdateExpression": "SET seen = :seen",
               "ConditionExpression": "tag <> :v",
               "ExpressionAttributeValues": {":seen": {"S": "yes"}, ":v": {"S": ""}}}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": "Sparse", "Key": {"id": {"S": "absent"}},
               "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["seen"],
        json!({"S": "yes"}),
        "case a conditional write gated by <>: the condition holds for an item with no \
         tag, so the update landed, got {read}"
    );
}

// ===========================================================================
// 2. The unknown-function refusal
// ===========================================================================

/// One unknown-function row: the case, the operation, the request, the expression member
/// the bad name travelled in, and the name itself.
struct FunctionRow {
    case: &'static str,
    operation: &'static str,
    request: Value,
    member: &'static str,
    name: &'static str,
}

/// The message the refusal must carry. The clause is AWS's template; the members a WRITE
/// states its intent through carry it inside the validation envelope this service words
/// every expression refusal of theirs in.
fn expected_function_message(member: &str, name: &str) -> String {
    let clause = format!("Invalid {member}: Invalid function name; function: {name}");
    if matches!(member, "ConditionExpression" | "UpdateExpression") {
        format!("1 validation error detected: {clause}")
    } else {
        clause
    }
}

/// The rows differ in the expression MEMBER and the operation carrying it, and in the
/// identifier: a misspelling of each documented function, one in the update grammar, and
/// a name that is no function of either grammar.
fn function_rows() -> Vec<FunctionRow> {
    vec![
        FunctionRow {
            case: "a misspelled attribute_exists in a Scan filter",
            operation: "Scan",
            request: json!({"TableName": "Functions", "ConsistentRead": true,
                            "FilterExpression": "attribute_exist(#a)",
                            "ExpressionAttributeNames": {"#a": "status"}}),
            member: "FilterExpression",
            name: "attribute_exist",
        },
        FunctionRow {
            case: "a camel-cased begins_with in a Scan filter",
            operation: "Scan",
            request: json!({"TableName": "Functions", "ConsistentRead": true,
                            "FilterExpression": "beginsWith(note, :v)",
                            "ExpressionAttributeValues": {":v": {"S": "a"}}}),
            member: "FilterExpression",
            name: "beginsWith",
        },
        FunctionRow {
            case: "a function of no grammar in a Query filter",
            operation: "Query",
            request: json!({"TableName": "Functions",
                            "KeyConditionExpression": "id = :id",
                            "FilterExpression": "matches(note, :v)",
                            "ExpressionAttributeValues": {":id": {"S": "one"},
                                                          ":v": {"S": "a"}}}),
            member: "FilterExpression",
            name: "matches",
        },
        FunctionRow {
            case: "a misspelled attribute_not_exists in a PutItem condition",
            operation: "PutItem",
            request: json!({"TableName": "Functions", "Item": {"id": {"S": "one"}},
                            "ConditionExpression": "attribute_not_exist(note)"}),
            member: "ConditionExpression",
            name: "attribute_not_exist",
        },
        FunctionRow {
            case: "a misspelled contains in a DeleteItem condition",
            operation: "DeleteItem",
            request: json!({"TableName": "Functions", "Key": {"id": {"S": "one"}},
                            "ConditionExpression": "contain(note, :v)",
                            "ExpressionAttributeValues": {":v": {"S": "a"}}}),
            member: "ConditionExpression",
            name: "contain",
        },
        FunctionRow {
            case: "a misspelled list_append in an UpdateExpression",
            operation: "UpdateItem",
            request: json!({"TableName": "Functions", "Key": {"id": {"S": "one"}},
                            "UpdateExpression": "SET l = list_appnd(l, :v)",
                            "ExpressionAttributeValues": {":v": {"L": []}}}),
            member: "UpdateExpression",
            name: "list_appnd",
        },
        FunctionRow {
            case: "a misspelled if_not_exists in an UpdateExpression",
            operation: "UpdateItem",
            request: json!({"TableName": "Functions", "Key": {"id": {"S": "one"}},
                            "UpdateExpression": "SET n = if_not_exist(n, :v)",
                            "ExpressionAttributeValues": {":v": {"N": "1"}}}),
            member: "UpdateExpression",
            name: "if_not_exist",
        },
    ]
}

#[tokio::test]
async fn an_unknown_function_name_is_refused_with_the_member_qualified_template() {
    let router = emulator();
    table_with(
        &router,
        "Functions",
        &[("id", "HASH", "S")],
        json!({"BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Functions",
               "Item": {"id": {"S": "one"}, "note": {"S": "alpha"}}}),
    )
    .await;

    for row in function_rows() {
        let (status, body) = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: an unknown function is a 400 ValidationException, got {status} {body}",
            row.case
        );
        let expected = expected_function_message(row.member, row.name);
        assert_eq!(
            message(&body),
            expected,
            "case {:?}: expected the message {expected:?}, got {body}",
            row.case
        );
    }

    // The documented functions are still served, so the refusal above is the rule and
    // not a blanket refusal of a call in an expression.
    for expression in [
        "attribute_exists(note)",
        "attribute_not_exists(nowhere)",
        "attribute_type(note, :s)",
        "begins_with(note, :a)",
        "contains(note, :a)",
        "size(note) > :zero",
    ] {
        let mut values = Map::new();
        for (token, value) in [
            (":s", json!({"S": "S"})),
            (":a", json!({"S": "a"})),
            (":zero", json!({"N": "0"})),
        ] {
            if expression.contains(token) {
                values.insert(token.to_string(), value);
            }
        }
        let mut body = json!({"TableName": "Functions", "ConsistentRead": true,
                              "FilterExpression": expression});
        if !values.is_empty() {
            body["ExpressionAttributeValues"] = Value::Object(values);
        }
        ok(&router, "Scan", body).await;
    }
}

// ===========================================================================
// 3. Every substitution token a request defines must be referenced
// ===========================================================================

/// One unused-token row: the case, the operation, the request, the expression member
/// the request states its intent through (empty when it carries none), the member that
/// carried the unused keys, and those keys as the message lists them.
struct UnusedRow {
    case: &'static str,
    operation: &'static str,
    request: Value,
    expression_member: &'static str,
    member: &'static str,
    keys: &'static str,
}

/// The rows differ in WHICH substitution table is unused, in how many of its keys are,
/// and in the operation and expression member that ignored them.
fn unused_rows() -> Vec<UnusedRow> {
    vec![
        UnusedRow {
            case: "a Scan whose filter references one of two names",
            operation: "Scan",
            request: json!({"TableName": "Unused", "ConsistentRead": true,
                            "FilterExpression": "attribute_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status",
                                                         "#b": "category"}}),
            expression_member: "FilterExpression",
            member: "ExpressionAttributeNames",
            keys: "#b",
        },
        UnusedRow {
            case: "a Scan with two unreferenced names",
            operation: "Scan",
            request: json!({"TableName": "Unused", "ConsistentRead": true,
                            "FilterExpression": "attribute_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status",
                                                         "#b": "category",
                                                         "#c": "note"}}),
            expression_member: "FilterExpression",
            member: "ExpressionAttributeNames",
            keys: "#b, #c",
        },
        UnusedRow {
            case: "a Scan whose filter references one of two values",
            operation: "Scan",
            request: json!({"TableName": "Unused", "ConsistentRead": true,
                            "FilterExpression": "label = :a",
                            "ExpressionAttributeValues": {":a": {"S": "OPEN"},
                                                          ":b": {"S": "DONE"}}}),
            expression_member: "FilterExpression",
            member: "ExpressionAttributeValues",
            keys: ":b",
        },
        UnusedRow {
            case: "a Scan whose projection leaves a name unreferenced",
            operation: "Scan",
            request: json!({"TableName": "Unused", "ConsistentRead": true,
                            "ProjectionExpression": "#a",
                            "ExpressionAttributeNames": {"#a": "status",
                                                         "#b": "category"}}),
            expression_member: "ProjectionExpression",
            member: "ExpressionAttributeNames",
            keys: "#b",
        },
        UnusedRow {
            case: "a Query whose key condition leaves a value unreferenced",
            operation: "Query",
            request: json!({"TableName": "Unused",
                            "KeyConditionExpression": "id = :a",
                            "ExpressionAttributeValues": {":a": {"S": "one"},
                                                          ":b": {"S": "two"}}}),
            expression_member: "KeyConditionExpression",
            member: "ExpressionAttributeValues",
            keys: ":b",
        },
        UnusedRow {
            case: "a GetItem with a name its projection never references",
            operation: "GetItem",
            request: json!({"TableName": "Unused", "Key": {"id": {"S": "one"}},
                            "ProjectionExpression": "#a",
                            "ExpressionAttributeNames": {"#a": "label", "#b": "status"}}),
            expression_member: "ProjectionExpression",
            member: "ExpressionAttributeNames",
            keys: "#b",
        },
        UnusedRow {
            case: "a PutItem whose condition leaves a value unreferenced",
            operation: "PutItem",
            request: json!({"TableName": "Unused", "Item": {"id": {"S": "one"}},
                            "ConditionExpression": "attribute_not_exists(#a)",
                            "ExpressionAttributeNames": {"#a": "status"},
                            "ExpressionAttributeValues": {":b": {"S": "DONE"}}}),
            expression_member: "ConditionExpression",
            member: "ExpressionAttributeValues",
            keys: ":b",
        },
        UnusedRow {
            case: "an UpdateItem whose update and condition leave a name unreferenced",
            operation: "UpdateItem",
            request: json!({"TableName": "Unused", "Key": {"id": {"S": "one"}},
                            "UpdateExpression": "SET #a = :a",
                            "ConditionExpression": "attribute_exists(id)",
                            "ExpressionAttributeNames": {"#a": "status",
                                                         "#b": "category"},
                            "ExpressionAttributeValues": {":a": {"S": "OPEN"}}}),
            expression_member: "UpdateExpression",
            member: "ExpressionAttributeNames",
            keys: "#b",
        },
        UnusedRow {
            case: "a DeleteItem whose condition leaves a value unreferenced",
            operation: "DeleteItem",
            request: json!({"TableName": "Unused", "Key": {"id": {"S": "one"}},
                            "ConditionExpression": "attribute_exists(id)",
                            "ExpressionAttributeValues": {":a": {"S": "OPEN"}}}),
            expression_member: "ConditionExpression",
            member: "ExpressionAttributeValues",
            keys: ":a",
        },
    ]
}

#[tokio::test]
async fn a_substitution_token_no_expression_references_is_refused() {
    let router = emulator();
    table_with(
        &router,
        "Unused",
        &[("id", "HASH", "S")],
        json!({"BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Unused",
               "Item": {"id": {"S": "one"}, "label": {"S": "OPEN"}}}),
    )
    .await;

    for row in unused_rows() {
        let (status, body) = call(&router, row.operation, row.request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: an unreferenced substitution token is a 400 ValidationException, \
             got {status} {body}",
            row.case
        );
        let expected = crate::undefined_expression_tokens::unused_entries(
            row.expression_member,
            row.member,
            &[row.keys],
        );
        assert_eq!(
            message(&body),
            expected,
            "case {:?}: expected the message {expected:?}, got {body}",
            row.case
        );
    }

    // The same requests with every token referenced are served: the refusal is about the
    // UNUSED key and not about carrying the member at all.
    for request in [
        json!({"TableName": "Unused", "ConsistentRead": true,
               "FilterExpression": "attribute_exists(#a) AND attribute_exists(#b)",
               "ExpressionAttributeNames": {"#a": "label", "#b": "id"}}),
        json!({"TableName": "Unused", "ConsistentRead": true,
               "FilterExpression": "#a = :a OR #a = :b",
               "ExpressionAttributeNames": {"#a": "label"},
               "ExpressionAttributeValues": {":a": {"S": "OPEN"}, ":b": {"S": "DONE"}}}),
    ] {
        ok(&router, "Scan", request).await;
    }
}

// ===========================================================================
// 4. What a PartiQL SELECT is charged for is what its plan read
// ===========================================================================

/// How many partitions the billed table holds, and how many items each holds.
const BILLED_PARTITIONS: usize = 4;
const BILLED_ITEMS_PER_PARTITION: usize = 5;

/// The `CapacityUnits` a response reported.
fn billed(answered: &Value) -> f64 {
    answered["ConsumedCapacity"]["CapacityUnits"]
        .as_f64()
        .unwrap_or_else(|| panic!("the response reports CapacityUnits, got {answered}"))
}

/// One statement row: the case, the statement, its parameters, how the capacity it
/// reports must relate to the native reads, and how many items it must report.
struct StatementRow {
    case: &'static str,
    statement: &'static str,
    parameters: Value,
    /// Which native read's capacity this statement must bill exactly.
    bills: Plan,
    items: usize,
}

/// The low-level plan a statement is served by, named by the native read that bills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// One item addressed by its whole primary key.
    Item,
    /// One partition, read by its partition key.
    Query,
    /// Every item of the table.
    Scan,
}

/// The rows differ in the FORM of the `WHERE` clause: a partition-key equality, the same
/// with a non-key predicate beside it, the whole primary key, a non-key predicate alone
/// (matching items and matching none), a disjunction that pins nothing, and no clause at
/// all.
fn statement_rows() -> Vec<StatementRow> {
    vec![
        StatementRow {
            case: "a partition-key equality",
            statement: "SELECT * FROM \"Billed\" WHERE pk = ?",
            parameters: json!([{"S": "p0"}]),
            bills: Plan::Query,
            items: BILLED_ITEMS_PER_PARTITION,
        },
        StatementRow {
            case: "a partition-key equality with a non-key predicate beside it",
            statement: "SELECT * FROM \"Billed\" WHERE pk = ? AND tag = ?",
            parameters: json!([{"S": "p0"}, {"S": "p0"}]),
            bills: Plan::Query,
            items: BILLED_ITEMS_PER_PARTITION,
        },
        StatementRow {
            case: "a partition-key equality whose non-key predicate matches nothing",
            statement: "SELECT * FROM \"Billed\" WHERE pk = ? AND tag = ?",
            parameters: json!([{"S": "p0"}, {"S": "nothing"}]),
            bills: Plan::Query,
            items: 0,
        },
        StatementRow {
            case: "the whole primary key",
            statement: "SELECT * FROM \"Billed\" WHERE pk = ? AND sk = ?",
            parameters: json!([{"S": "p0"}, {"N": "0"}]),
            bills: Plan::Item,
            items: 1,
        },
        StatementRow {
            case: "a non-key equality, which pins no partition",
            statement: "SELECT * FROM \"Billed\" WHERE tag = ?",
            parameters: json!([{"S": "p0"}]),
            bills: Plan::Scan,
            items: BILLED_ITEMS_PER_PARTITION,
        },
        StatementRow {
            case: "a non-key equality matching nothing",
            statement: "SELECT * FROM \"Billed\" WHERE tag = ?",
            parameters: json!([{"S": "nothing"}]),
            bills: Plan::Scan,
            items: 0,
        },
        StatementRow {
            case: "a disjunction that pins no partition key",
            statement: "SELECT * FROM \"Billed\" WHERE pk = ? OR tag = ?",
            parameters: json!([{"S": "p0"}, {"S": "p1"}]),
            bills: Plan::Scan,
            items: 2 * BILLED_ITEMS_PER_PARTITION,
        },
        StatementRow {
            case: "no predicate at all",
            statement: "SELECT * FROM \"Billed\"",
            parameters: Value::Null,
            bills: Plan::Scan,
            items: BILLED_PARTITIONS * BILLED_ITEMS_PER_PARTITION,
        },
    ]
}

#[tokio::test]
async fn a_statement_is_charged_for_what_its_plan_read() {
    let router = emulator();
    table_with(
        &router,
        "Billed",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        json!({"BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    // Items of about a kilobyte each, so a partition's read and the table's differ by
    // more than one capacity unit.
    let fill = "f".repeat(1000);
    for partition in 0..BILLED_PARTITIONS {
        for sort in 0..BILLED_ITEMS_PER_PARTITION {
            ok(
                &router,
                "PutItem",
                json!({"TableName": "Billed", "Item": {
                    "pk": {"S": format!("p{partition}")},
                    "sk": {"N": sort.to_string()},
                    "tag": {"S": format!("p{partition}")},
                    "fill": {"S": fill},
                }}),
            )
            .await;
        }
    }

    // What each plan costs, measured through the native read that IS that plan.
    let item_read = billed(
        &ok(
            &router,
            "GetItem",
            json!({"TableName": "Billed", "Key": {"pk": {"S": "p0"}, "sk": {"N": "0"}},
                   "ConsistentRead": true, "ReturnConsumedCapacity": "TOTAL"}),
        )
        .await,
    );
    let query_read = billed(
        &ok(
            &router,
            "Query",
            json!({"TableName": "Billed", "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "p0"}},
                   "ConsistentRead": true, "ReturnConsumedCapacity": "TOTAL"}),
        )
        .await,
    );
    let scan_read = billed(
        &ok(
            &router,
            "Scan",
            json!({"TableName": "Billed", "ConsistentRead": true,
                   "ReturnConsumedCapacity": "TOTAL"}),
        )
        .await,
    );
    let filtered_scan = billed(
        &ok(
            &router,
            "Scan",
            json!({"TableName": "Billed", "FilterExpression": "tag = :p",
                   "ExpressionAttributeValues": {":p": {"S": "p0"}},
                   "ConsistentRead": true, "ReturnConsumedCapacity": "TOTAL"}),
        )
        .await,
    );
    assert_eq!(
        filtered_scan, scan_read,
        "case the native reads: a FilterExpression never reduces what a scan read, got \
         {filtered_scan} against {scan_read}"
    );
    assert!(
        item_read < query_read && query_read < scan_read,
        "case the native reads: the three plans must cost strictly more as they read \
         more, got item {item_read}, query {query_read}, scan {scan_read}"
    );

    for row in statement_rows() {
        let mut body = json!({"TableName": "Billed", "Statement": row.statement,
                              "ConsistentRead": true, "ReturnConsumedCapacity": "TOTAL"});
        if !row.parameters.is_null() {
            body["Parameters"] = row.parameters.clone();
        }
        let answered = ok(&router, "ExecuteStatement", body).await;
        let expected = match row.bills {
            Plan::Item => item_read,
            Plan::Query => query_read,
            Plan::Scan => scan_read,
        };
        assert_eq!(
            billed(&answered),
            expected,
            "case {:?}: expected the {:?} plan's {expected} units, got {answered}",
            row.case,
            row.bills
        );
        assert_eq!(
            answered["Items"].as_array().map(Vec::len).unwrap_or(0),
            row.items,
            "case {:?}: expected {} items, got {answered}",
            row.case,
            row.items
        );
    }

    // The two statements that report the SAME items differ only in cost, which is what
    // tells a client which plan it got.
    let by_key = ok(
        &router,
        "ExecuteStatement",
        json!({"TableName": "Billed", "ConsistentRead": true,
               "ReturnConsumedCapacity": "TOTAL",
               "Statement": "SELECT * FROM \"Billed\" WHERE pk = ?",
               "Parameters": [{"S": "p0"}]}),
    )
    .await;
    let by_tag = ok(
        &router,
        "ExecuteStatement",
        json!({"TableName": "Billed", "ConsistentRead": true,
               "ReturnConsumedCapacity": "TOTAL",
               "Statement": "SELECT * FROM \"Billed\" WHERE tag = ?",
               "Parameters": [{"S": "p0"}]}),
    )
    .await;
    let addressed = |answered: &Value| {
        let mut keys: Vec<String> = answered["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| format!("{}/{}", item["pk"]["S"], item["sk"]["N"]))
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(
        addressed(&by_key),
        addressed(&by_tag),
        "case the same item set: both statements report the same items, got {by_key} and \
         {by_tag}"
    );
    assert!(
        billed(&by_tag) > billed(&by_key),
        "case the same item set: the statement planned as a scan costs strictly more, got \
         {} against {}",
        billed(&by_tag),
        billed(&by_key)
    );
}

// ===========================================================================
// 5. A write is admitted from the table's capacity and from its key's
// ===========================================================================

/// An item of just under the documented 400 KB maximum, so one write of it consumes a
/// large share of a partition key's write ceiling and the arms below need a handful of
/// requests rather than thousands.
fn heavy_item(partition: &str, sort: usize, payload: &str) -> Value {
    json!({"pk": {"S": partition}, "sk": {"N": sort.to_string()},
           "blob": {"S": payload}})
}

/// Whether the key `(partition, sort)` holds an item, read consistently.
async fn stored(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    partition: &str,
    sort: usize,
) -> bool {
    let read = ok(
        router,
        "GetItem",
        json!({"TableName": table,
               "Key": {"pk": {"S": partition}, "sk": {"N": sort.to_string()}},
               "ConsistentRead": true}),
    )
    .await;
    read.get("Item").is_some()
}

#[tokio::test]
async fn a_write_stream_aimed_at_one_partition_key_is_throttled_and_stores_nothing() {
    let router = emulator();
    // Provisioned far above what these arms offer, so the TABLE's capacity is never the
    // binding constraint and what the arms differ in is the key they aim at.
    table_with(
        &router,
        "Hot",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 3000,
                                         "WriteCapacityUnits": 4000}}),
    )
    .await;
    let payload = "x".repeat(399 * 1024);

    // Arm one: every write aimed at ONE partition key. The key's own ceiling is reached
    // inside the arm, so at least one write is refused.
    let mut refused: Vec<(usize, Value)> = Vec::new();
    let mut landed: Vec<usize> = Vec::new();
    for sort in 0..4 {
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": "Hot", "Item": heavy_item("hot", sort, &payload)}),
        )
        .await;
        if status == 200 {
            landed.push(sort);
        } else {
            refused.push((sort, body));
        }
    }
    assert!(
        !refused.is_empty(),
        "case one key: the per-partition-key ceiling must refuse a write of the arm, all \
         {} landed",
        landed.len()
    );
    for (sort, body) in &refused {
        assert_eq!(
            error_code(body),
            "ProvisionedThroughputExceededException",
            "case one key, write {sort}: a throttled write is a \
             ProvisionedThroughputExceededException, got {body}"
        );
        assert_eq!(
            body["ThrottlingReasons"],
            json!([{"reason": "TableWriteKeyRangeThroughputExceeded",
                    "resource": arn_of("Hot")}]),
            "case one key, write {sort}: the reason names the key-range ceiling and the \
             table's ARN, got {body}"
        );
        assert!(
            !stored(&router, "Hot", "hot", *sort).await,
            "case one key, write {sort}: a throttled write stores nothing"
        );
    }
    for sort in &landed {
        assert!(
            stored(&router, "Hot", "hot", *sort).await,
            "case one key, write {sort}: an admitted write is stored"
        );
    }

    // Arm two: the same number of writes of the same size, spread over DISTINCT
    // partition keys — inside the table's budget and inside each key's, so every one of
    // them is served.
    for key in 0..4 {
        let partition = format!("spread-{key}");
        let answered = ok(
            &router,
            "PutItem",
            json!({"TableName": "Hot", "Item": heavy_item(&partition, 0, &payload)}),
        )
        .await;
        assert!(
            answered.get("ThrottlingReasons").is_none(),
            "case spread keys, key {key}: a write inside both ceilings is served, got \
             {answered}"
        );
        assert!(
            stored(&router, "Hot", &partition, 0).await,
            "case spread keys, key {key}: the write is stored"
        );
    }
}

#[tokio::test]
async fn a_batch_hands_back_the_writes_the_ceilings_did_not_admit() {
    let router = emulator();
    table_with(
        &router,
        "HotBatch",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 3000,
                                         "WriteCapacityUnits": 4000}}),
    )
    .await;
    let payload = "x".repeat(399 * 1024);

    // A STREAM of batches of eight ~400-unit writes aimed at ONE partition key. That key's
    // ceiling serves one call whole — it lends a call what one call can ask of a partition
    // — so what it bounds is the SUSTAINED rate: a caller that keeps batching past it runs
    // the allowance out, and from then on a call lands what is left and hands the rest
    // back.
    let batch_of = |round: usize| -> Vec<Value> {
        (0..8)
            .map(|slot| {
                let sort = round * 8 + slot;
                json!({"PutRequest": {"Item": heavy_item("hot", sort, &payload)}})
            })
            .collect()
    };
    let mut handed_back: Vec<Value> = Vec::new();
    let mut rounds = 0usize;
    for round in 0..8usize {
        let answered = ok(
            &router,
            "BatchWriteItem",
            json!({"TableName": "HotBatch",
                   "RequestItems": {"HotBatch": batch_of(round)},
                   "ReturnConsumedCapacity": "TOTAL"}),
        )
        .await;
        rounds = round + 1;
        handed_back = answered["UnprocessedItems"]["HotBatch"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !handed_back.is_empty() {
            assert!(
                handed_back.len() < 8,
                "case one key in a stream of batches, round {round}: the ceiling admits what \
                 it can and hands the rest back, got {answered}"
            );
            break;
        }
    }
    assert!(
        !handed_back.is_empty(),
        "case one key in a stream of batches: the key's ceiling bounds the sustained rate, \
         so a stream past it must hand something back, {rounds} rounds landed whole"
    );
    // Every operation the response handed back is absent from the store, and every one it
    // did not is there: `UnprocessedItems` is "in the same form as RequestItems", so the
    // sort key of each tells which write it was.
    let unprocessed_sorts: Vec<usize> = handed_back
        .iter()
        .map(|request| {
            request["PutRequest"]["Item"]["sk"]["N"]
                .as_str()
                .unwrap_or_default()
                .parse()
                .expect("the handed-back request carries its sort key")
        })
        .collect();
    for sort in 0..rounds * 8 {
        let present = stored(&router, "HotBatch", "hot", sort).await;
        assert_eq!(
            present,
            !unprocessed_sorts.contains(&sort),
            "case one key in a stream of batches, write {sort}: a handed-back write stores \
             nothing and an admitted one is stored, the residue held {unprocessed_sorts:?}"
        );
    }

    // A batch NONE of whose writes are admitted is answered with the throttle itself,
    // naming the ceiling that bound. The FIRST write is admitted while the table still has
    // capacity — the capacity a table is provisioned for is a sustained RATE, so a write
    // arriving with capacity left is served and charged what it consumed — and it is the
    // one after it, arriving on a budget that draw left in deficit, that gets no work done.
    table_with(
        &router,
        "Tiny",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 1,
                                         "WriteCapacityUnits": 1}}),
    )
    .await;
    let (status, drained) = call(
        &router,
        "BatchWriteItem",
        json!({"TableName": "Tiny",
               "RequestItems": {"Tiny": [
                   {"PutRequest": {"Item": heavy_item("first", 0, &payload)}}]}}),
    )
    .await;
    assert_eq!(
        (status, drained["UnprocessedItems"].clone()),
        (200, json!({})),
        "case the first write of a provisioned table: capacity is available, so the write \
         lands whole, got {status} {drained}"
    );
    let (status, body) = call(
        &router,
        "BatchWriteItem",
        json!({"TableName": "Tiny",
               "RequestItems": {"Tiny": [
                   {"PutRequest": {"Item": heavy_item("only", 0, &payload)}}]}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case nothing admitted: a batch that got no work done is answered with the \
         throttle, got {status} {body}"
    );
    assert_eq!(
        body["ThrottlingReasons"],
        json!([{"reason": "TableWriteProvisionedThroughputExceeded",
                "resource": arn_of("Tiny")}]),
        "case nothing admitted: the reason names the table's provisioning and its ARN, \
         got {body}"
    );
    assert!(
        !stored(&router, "Tiny", "only", 0).await,
        "case nothing admitted: the refused batch stored nothing"
    );

    // The single-item write path answers the same table-level throttle, with the same
    // reason, for the same table.
    let (status, single) = call(
        &router,
        "PutItem",
        json!({"TableName": "Tiny", "Item": heavy_item("only", 1, &payload)}),
    )
    .await;
    assert_eq!(
        (status, error_code(&single).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case the table ceiling on PutItem: expected the throttle, got {status} {single}"
    );
    assert_eq!(
        single["ThrottlingReasons"],
        json!([{"reason": "TableWriteProvisionedThroughputExceeded",
                "resource": arn_of("Tiny")}]),
        "case the table ceiling on PutItem: the reason names the table's provisioning, \
         got {single}"
    );

    // An UpdateItem is admitted from the same budget, and a refused one creates no item.
    let (status, updated) = call(
        &router,
        "UpdateItem",
        json!({"TableName": "Tiny", "Key": {"pk": {"S": "only"}, "sk": {"N": "2"}},
               "UpdateExpression": "SET payload = :b",
               "ExpressionAttributeValues": {":b": {"S": payload}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&updated).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case the table ceiling on UpdateItem: expected the throttle, got {status} {updated}"
    );
    assert_eq!(
        updated["ThrottlingReasons"],
        json!([{"reason": "TableWriteProvisionedThroughputExceeded",
                "resource": arn_of("Tiny")}]),
        "case the table ceiling on UpdateItem: the reason names the table's provisioning, \
         got {updated}"
    );
    assert!(
        !stored(&router, "Tiny", "only", 2).await,
        "case the table ceiling on UpdateItem: a throttled update stores nothing"
    );

    // A DELETE is a write too, and is admitted from the same budget: the item is stored
    // against capacity that covers it, the table is then held to one write unit — whose
    // whole burst is less than removing an item of this size costs — and the delete is
    // refused with the item still there afterwards.
    table_with(
        &router,
        "Snug",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 4000,
                                         "WriteCapacityUnits": 4000}}),
    )
    .await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Snug", "Item": heavy_item("one", 0, &payload)}),
    )
    .await;
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": "Snug", "BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 4000, "WriteCapacityUnits": 1}}),
    )
    .await;
    // The budget is drained at the rate now in force: one write of this size is admitted
    // while the table still has capacity, and takes far more than a one-unit table holds, so
    // the DELETE that follows arrives on the deficit it left.
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Snug", "Item": heavy_item("one", 1, &payload)}),
    )
    .await;
    let (status, deleted) = call(
        &router,
        "DeleteItem",
        json!({"TableName": "Snug", "Key": {"pk": {"S": "one"}, "sk": {"N": "0"}}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&deleted).as_str()),
        (400, "ProvisionedThroughputExceededException"),
        "case the table ceiling on DeleteItem: expected the throttle, got {status} {deleted}"
    );
    assert!(
        stored(&router, "Snug", "one", 0).await,
        "case the table ceiling on DeleteItem: a throttled delete removes nothing"
    );
}
