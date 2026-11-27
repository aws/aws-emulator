// SPDX-License-Identifier: CC0-1.0

//! The operand DOMAIN of the expression language's prefix test, over every operand whose
//! type the REQUEST settles, driven end to end.
//!
//! RULE under test: `begins_with` relates one text to another — "begins_with (path,
//! substr) — true if the attribute specified by path begins with a particular substring"
//! (the `ConditionExpression` function reference), and only the string and binary types
//! have a prefix — so an operand of any other type, in EITHER of its two positions, is a
//! defect of the REQUEST: HTTP 400 `ValidationException` naming the function and that
//! operand's type, settled before any item is read, and never a conditional-check
//! failure, a cancelled transaction or a page selecting nothing.
//!
//! An operand's type is settled by the request in two forms, and both are held to the
//! domain: a value the request bound to a `:placeholder`, and a `size (path)` call, which
//! "Returns a number representing an attribute's size" whatever the location it measures
//! holds — so it is a Number operand, reported as the type `N`. An operand that READS an
//! attribute carries no type the request states, so it is left to the evaluation; the
//! control at the end drives those forms and the two types the function IS defined over,
//! so no row passes by refusing every prefix test.
//!
//! The rows differ along what the rule generalises over: the operand POSITION (the tested
//! text and the prefix), the operand FORM (a bound value and a `size` call), the TYPE of
//! the offending operand (every `AttributeValue` member that is neither `S` nor `B`), the
//! expression MEMBER the text arrived in (`ConditionExpression`, `FilterExpression`), and
//! the operation carrying it (the three single-item writes, both table walks, and the
//! PartiQL statement surface).
//!
//! The type codes come from this service's Smithy model, the members of the
//! `AttributeValue` union (`smithy_query(query="shape", shape="AttributeValue")`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row addresses: one string partition key.
const TABLE: &str = "BeginsWithOperandDomain";

/// The envelope a refusal about a WRITE's own intent member is stated inside, which this
/// service states every condition-expression clause of a single-item write in.
const ENVELOPE: &str = "1 validation error detected: ";

/// The key of the one seeded item, which every write row addresses.
fn key() -> Value {
    json!({"pk": {"S": "k1"}})
}

/// The emulator with the table and one item in place, so no row is refused for its table
/// or its key instead of for the operand domain under test.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({"TableName": TABLE, "BillingMode": "PAY_PER_REQUEST",
               "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
               "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}]}),
    )
    .await;
    describe_when_active(&router, TABLE, TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE,
               "Item": {"pk": {"S": "k1"}, "s": {"S": "beta"}, "peer": {"S": "beta"},
                        "n": {"N": "3"},
                        "bin": {"B": ::aws_smithy_types::base64::encode([1u8, 2])}}}),
    )
    .await;
    router
}

/// AWS's fixed sentence for an operand whose type the function it was given to is not
/// defined over, with the function and that type substituted.
fn operand_type(datatype: &str) -> String {
    format!(
        "Incorrect operand type for operator or function; operator or function: begins_with, \
         operand type: {datatype}"
    )
}

/// The whole message an expression `member` of a request earns for that clause.
fn clause(member: &str, datatype: &str, enveloped: bool) -> String {
    let envelope = if enveloped { ENVELOPE } else { "" };
    format!("{envelope}Invalid {member}: {}", operand_type(datatype))
}

/// Every `AttributeValue` type a prefix test is NOT defined over, each with the value a
/// request binds it as — the members of the `AttributeValue` union except `S` and `B`,
/// which are the two types that have a prefix.
fn values_without_a_prefix() -> Vec<(&'static str, Value)> {
    vec![
        ("N", json!({"N": "1"})),
        ("BOOL", json!({"BOOL": true})),
        ("NULL", json!({"NULL": true})),
        ("L", json!({"L": [{"S": "a"}]})),
        ("M", json!({"M": {"a": {"S": "b"}}})),
        ("SS", json!({"SS": ["a"]})),
        ("NS", json!({"NS": ["1"]})),
        ("BS", json!({"BS": ["YQ=="]})),
    ]
}

/// Drive one request that must be refused as the request's own defect, answering the
/// message it was refused with.
async fn refused(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> String {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
    );
    message(&answered)
}

/// The request each conditional write sends with `expression` as its condition: the
/// payload beside the condition that its own model requires.
fn write_request(operation: &str, expression: &str, values: Value) -> Value {
    let mut request = match operation {
        "PutItem" => json!({"Item": {"pk": {"S": "k1"}, "s": {"S": "written"}}}),
        "UpdateItem" => json!({"Key": key(), "UpdateExpression": "SET s = :written"}),
        _ => json!({"Key": key()}),
    };
    request["TableName"] = json!(TABLE);
    request["ConditionExpression"] = json!(expression);
    let mut bound = values;
    if operation == "UpdateItem" {
        bound[":written"] = json!({"S": "written"});
    }
    // A substitution table the request has nothing to put in it is not sent at all: an
    // empty one is its own defect, which is not the rule under test.
    if bound.as_object().is_some_and(|bound| !bound.is_empty()) {
        request["ExpressionAttributeValues"] = bound;
    }
    request
}

/// RULE: a bound operand of a type that has no prefix is refused, in EITHER position, on
/// every single-item write that states a condition.
#[tokio::test]
async fn a_conditional_write_refuses_a_prefix_test_operand_of_a_type_with_no_prefix() {
    let router = seeded().await;

    for operation in ["PutItem", "UpdateItem", "DeleteItem"] {
        for (datatype, value) in values_without_a_prefix() {
            let expected = clause("ConditionExpression", datatype, true);

            // The offending operand as the PREFIX, which the reference calls `substr`…
            let case = format!("{operation} with a {datatype} prefix");
            let stated = refused(
                &router,
                &case,
                operation,
                write_request(operation, "begins_with(s, :v)", json!({":v": value.clone()})),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );

            // …and as the TESTED TEXT, which is the same domain: a value with no prefix
            // cannot begin with anything either.
            let case = format!("{operation} testing a {datatype} value for a prefix");
            let stated = refused(
                &router,
                &case,
                operation,
                write_request(operation, "begins_with(:v, s)", json!({":v": value})),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    // The seeded item is untouched: every row above was refused before anything was
    // written.
    let stored = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        stored["Item"]["s"],
        json!({"S": "beta"}),
        "case the seeded item: a refused conditional write stores nothing, got {stored}"
    );
}

/// RULE: a `size (path)` operand is a NUMBER whatever the location it measures holds, so
/// a prefix test given one is refused for the type `N` — in either position, in both
/// expression members, and on every operation that carries one.
#[tokio::test]
async fn a_size_operand_is_a_number_wherever_a_prefix_test_reads_it() {
    let router = seeded().await;

    // (case, operation, the whole request, the member its clause names, enveloped)
    let rows: Vec<(&str, &str, Value, &str, bool)> = vec![
        (
            "a put's condition testing a size for a prefix",
            "PutItem",
            write_request("PutItem", "begins_with(size(s), :v)", json!({":v": {"S": "4"}})),
            "ConditionExpression",
            true,
        ),
        (
            "an update's condition reading a size as the prefix",
            "UpdateItem",
            write_request("UpdateItem", "begins_with(s, size(s))", json!({})),
            "ConditionExpression",
            true,
        ),
        (
            "a delete's condition testing a size for a prefix",
            "DeleteItem",
            write_request(
                "DeleteItem",
                "begins_with(size(s), :v)",
                json!({":v": {"S": "4"}}),
            ),
            "ConditionExpression",
            true,
        ),
        (
            "a delete's condition reading a size over a BINARY attribute",
            "DeleteItem",
            write_request(
                "DeleteItem",
                "begins_with(size(bin), :v)",
                json!({":v": {"S": "2"}}),
            ),
            "ConditionExpression",
            true,
        ),
        (
            "a scan filtering on a size as the prefix",
            "Scan",
            json!({"TableName": TABLE, "FilterExpression": "begins_with(s, size(peer))"}),
            "FilterExpression",
            false,
        ),
        (
            "a query filtering on a size as the tested text",
            "Query",
            json!({"TableName": TABLE,
                   "KeyConditionExpression": "pk = :p",
                   "FilterExpression": "begins_with(size(s), :v)",
                   "ExpressionAttributeValues": {":p": {"S": "k1"}, ":v": {"S": "4"}}}),
            "FilterExpression",
            false,
        ),
    ];

    for (case, operation, request, member, enveloped) in rows {
        let stated = refused(&router, case, operation, request).await;
        let expected = clause(member, "N", enveloped);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE: the same domain holds for a READ's filter, where the clause is stated on its own
/// — one row per type a prefix test is not defined over, in both operand positions, over
/// both table walks.
#[tokio::test]
async fn a_reads_filter_refuses_a_prefix_test_operand_of_a_type_with_no_prefix() {
    let router = seeded().await;

    for (datatype, value) in values_without_a_prefix() {
        for (position, expression) in [
            ("the prefix", "begins_with(s, :v)"),
            ("the tested text", "begins_with(:v, s)"),
        ] {
            let expected = clause("FilterExpression", datatype, false);

            let case = format!("a scan with a {datatype} operand as {position}");
            let stated = refused(
                &router,
                &case,
                "Scan",
                json!({"TableName": TABLE, "FilterExpression": expression,
                       "ExpressionAttributeValues": {":v": value.clone()}}),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );

            let case = format!("a query with a {datatype} operand as {position}");
            let stated = refused(
                &router,
                &case,
                "Query",
                json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p",
                       "FilterExpression": expression,
                       "ExpressionAttributeValues": {":p": {"S": "k1"},
                                                     ":v": value.clone()}}),
            )
            .await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}

/// RULE: the PartiQL statement surface holds the same operand domain, stating the clause
/// the way it states every other refusal about a statement.
#[tokio::test]
async fn a_statements_prefix_test_refuses_an_operand_of_a_type_with_no_prefix() {
    let router = seeded().await;

    // The literal a statement spells, per type it may spell: the bags that denote a set
    // and the keyword literals included, and `BS` through a bound parameter, which no
    // literal spells.
    for (datatype, literal, parameters) in [
        ("N", "1", None),
        ("BOOL", "true", None),
        ("NULL", "null", None),
        ("L", "[1]", None),
        ("M", "{'a': 1}", None),
        ("SS", "<<'a'>>", None),
        ("NS", "<<1>>", None),
        ("BS", "?", Some(json!([{"BS": ["YQ=="]}]))),
    ] {
        let statement = format!("SELECT * FROM \"{TABLE}\" WHERE begins_with(s, {literal})");
        let mut body = json!({"Statement": statement});
        if let Some(parameters) = parameters {
            body["Parameters"] = parameters;
        }
        let case = format!("a statement whose prefix is a {datatype}");
        let stated = refused(&router, &case, "ExecuteStatement", body).await;
        let expected = operand_type(datatype);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }

    // The other operand form whose type the STATEMENT settles: `size (path)` reports a
    // number whatever the location it measures holds, so a prefix test given one is
    // refused for `N` on this surface exactly as it is on the expression surface.
    for (case, statement) in [
        (
            "a statement reading a size as the prefix",
            format!("SELECT * FROM \"{TABLE}\" WHERE begins_with(s, size(s))"),
        ),
        (
            "a statement reading a size over a binary attribute as the prefix",
            format!("SELECT * FROM \"{TABLE}\" WHERE begins_with(s, size(bin))"),
        ),
    ] {
        let stated = refused(
            &router,
            case,
            "ExecuteStatement",
            json!({"Statement": statement}),
        )
        .await;
        let expected = operand_type("N");
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// The CONTROL for every refusal above: the two types a prefix test IS defined over are
/// served in both operand positions, an operand that READS an attribute carries no type
/// the request states — so it is evaluated against the item rather than refused — and a
/// `size` operand given to an operator that IS defined over numbers is served.
#[tokio::test]
async fn a_prefix_test_over_the_types_that_have_a_prefix_is_evaluated() {
    let router = seeded().await;
    let binary = ::aws_smithy_types::base64::encode([1u8]);

    // (case, the filter, the substitution table, how many of the one seeded item it
    // selects) — a filter that selects nothing is still a SERVED page.
    let rows: Vec<(&str, &str, Value, i64)> = vec![
        (
            "a string prefix that holds",
            "begins_with(s, :v)",
            json!({":v": {"S": "bet"}}),
            1,
        ),
        (
            "a string prefix that does not hold",
            "begins_with(s, :v)",
            json!({":v": {"S": "zzz"}}),
            0,
        ),
        (
            "a binary prefix that holds",
            "begins_with(bin, :v)",
            json!({":v": {"B": binary}}),
            1,
        ),
        (
            "a value the request bound tested for a prefix the item holds",
            "begins_with(:v, s)",
            json!({":v": {"S": "betas"}}),
            1,
        ),
        (
            "two operands both read off the item",
            "begins_with(s, peer)",
            json!({}),
            1,
        ),
        (
            "a prefix read from a location the test does not hold for",
            "begins_with(s, pk)",
            json!({}),
            0,
        ),
        (
            "a size compared with a number, which numbers ARE ordered by",
            "size(s) = :four",
            json!({":four": {"N": "4"}}),
            1,
        ),
    ];

    for (case, filter, values, selected) in rows {
        let mut request = json!({"TableName": TABLE, "FilterExpression": filter});
        if values.as_object().is_some_and(|bound| !bound.is_empty()) {
            request["ExpressionAttributeValues"] = values;
        }
        let page = ok(&router, "Scan", request).await;
        assert_eq!(
            page["Count"],
            json!(selected),
            "case {case:?}: expected {selected} item(s) selected, got {page}"
        );
    }

    // And a statement whose prefix IS a string is served, so the statement rows above are
    // this domain rather than a blanket refusal of every prefix test.
    let served = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": format!("SELECT * FROM \"{TABLE}\" WHERE begins_with(s, 'bet')")}),
    )
    .await;
    assert_eq!(
        served["Items"].as_array().map(Vec::len),
        Some(1),
        "case a statement whose prefix is a string: expected the seeded item, got {served}"
    );

    // A conditional write whose prefix test HOLDS is applied: the delete removes the item
    // it guarded, so the write rows above are this domain and not a blanket refusal.
    ok(
        &router,
        "DeleteItem",
        write_request("DeleteItem", "begins_with(s, :v)", json!({":v": {"S": "bet"}})),
    )
    .await;
    let stored = ok(&router, "GetItem", json!({"TableName": TABLE, "Key": key()})).await;
    assert_eq!(
        stored.get("Item"),
        None,
        "case a condition that holds: the delete removes the item, got {stored}"
    );
}
