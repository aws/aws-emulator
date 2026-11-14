// SPDX-License-Identifier: CC0-1.0

//! What a KEY condition and the legacy condition maps accept, and what they answer when
//! they do not.
//!
//! The rules these suites pin:
//!
//! * a `KeyConditionExpression` operand is held to the `AttributeValue` datatypes the
//!   operator or function it was written with compares — the three scalar key types for a
//!   comparison, String or Binary for `begins_with` — BEFORE it is held to the type the
//!   table declared for the key it is compared with, and the refusal names the operator
//!   as the expression WROTE it;
//! * either side of a comparison may carry the operand: `:v >= sk` states about the
//!   attribute exactly what `sk <= :v` does;
//! * every defect is reported where the reading of the expression reaches it, so the
//!   defect an expression states FIRST is the one answered, and a syntax error names the
//!   token the parse stopped at;
//! * a key condition over a schema declaring several RANGE key attributes constrains them
//!   from the first: a condition on one of them needs an equality on every range key
//!   attribute the schema declares before it, and the refusal names them;
//! * the legacy `KeyConditions` map states only the comparisons an index can be read
//!   through, and names any other as the constraint that is not an indexable operation;
//! * a legacy `Expected` entry takes the value it compares against from
//!   `AttributeValueList` or from `Value`, and only the member pairs that state the same
//!   half of the condition twice cannot be used together;
//! * a `Key` member's value is held to the documented rules for its own TYPE — a set may
//!   not be empty or list one member twice — before it is held to the key schema.
//!
//! Each suite checks its rule over a TABLE of inputs rather than one request: every
//! datatype the `AttributeValue` shape declares against every comparison a key condition
//! takes, both operand orders of each comparison, both orders of two defects in one
//! expression, each position a composite range key can be constrained at, every
//! comparison the `ComparisonOperator` enum declares, and the three set types on a key
//! across a read and a write.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table the key-condition suites read: a composite primary key whose sort key is a
/// String, so an operand of another type is both a datatype the operator may not compare
/// and a type the schema does not declare — which is what makes the ORDER of the two
/// rules visible.
const TABLE: &str = "KeyConditionRules";

/// The global secondary index with a COMPOSITE range key: one partition key and the four
/// sort keys "each global secondary index supports up to 4 partition keys and up to 4 sort
/// keys" (`CreateTable$GlobalSecondaryIndexes`) allows.
const COMPOSITE_INDEX: &str = "ByComposite";

/// AWS's fixed clause for an operand whose type the operator does not compare, with the
/// operator as written and the datatype substituted.
fn incorrect_operand(operator: &str, datatype: &str) -> String {
    format!(
        "Invalid KeyConditionExpression: Incorrect operand type for operator or function; \
         operator or function: {operator}, operand type: {datatype}"
    )
}

/// AWS's fixed sentence for an operand whose type the key's SCHEMA does not declare,
/// which names neither the attribute nor either type.
const SCHEMA_TYPE: &str =
    "One or more parameter values were invalid: Condition parameter type does not match \
     schema type";

/// Call `operation`, require a 400 `ValidationException`, and answer its message.
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

/// Create [`TABLE`] with [`COMPOSITE_INDEX`] over it, wait out the documented
/// `CREATING` -> `ACTIVE` transition, and seed the items every suite here reads.
async fn seeded(router: &MultiServiceRouter) {
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
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gpk", "AttributeType": "S"},
                {"AttributeName": "gsk1", "AttributeType": "S"},
                {"AttributeName": "gsk2", "AttributeType": "S"},
                {"AttributeName": "gsk3", "AttributeType": "N"},
                {"AttributeName": "gsk4", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": COMPOSITE_INDEX,
                "KeySchema": [
                    {"AttributeName": "gpk", "KeyType": "HASH"},
                    {"AttributeName": "gsk1", "KeyType": "RANGE"},
                    {"AttributeName": "gsk2", "KeyType": "RANGE"},
                    {"AttributeName": "gsk3", "KeyType": "RANGE"},
                    {"AttributeName": "gsk4", "KeyType": "RANGE"},
                ],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await;
    describe_when_active(router, "seed the key-condition table", TABLE).await;
    for item in stored_items() {
        ok(router, "PutItem", json!({"TableName": TABLE, "Item": item})).await;
    }
}

/// The items the suites read: one partition of four sort keys, each also carrying the
/// composite index's five key attributes, and one item that carries none of them (so the
/// index does not cover it).
fn stored_items() -> Vec<Value> {
    vec![
        json!({"pk": {"S": "p"}, "sk": {"S": "a"}, "note": {"S": "first"},
               "gpk": {"S": "g"}, "gsk1": {"S": "one"}, "gsk2": {"S": "two"},
               "gsk3": {"N": "3"}, "gsk4": {"S": "alpha"}}),
        json!({"pk": {"S": "p"}, "sk": {"S": "b"}, "note": {"S": "second"},
               "gpk": {"S": "g"}, "gsk1": {"S": "one"}, "gsk2": {"S": "two"},
               "gsk3": {"N": "3"}, "gsk4": {"S": "beta"}}),
        json!({"pk": {"S": "p"}, "sk": {"S": "c"}, "note": {"S": "third"},
               "gpk": {"S": "g"}, "gsk1": {"S": "one"}, "gsk2": {"S": "other"},
               "gsk3": {"N": "4"}, "gsk4": {"S": "gamma"}}),
        json!({"pk": {"S": "p"}, "sk": {"S": "d"}, "note": {"S": "fourth"},
               "gpk": {"S": "g"}, "gsk1": {"S": "two"}, "gsk2": {"S": "two"},
               "gsk3": {"N": "5"}, "gsk4": {"S": "delta"}}),
        json!({"pk": {"S": "q"}, "sk": {"S": "a"}, "note": {"S": "fifth"}}),
    ]
}

/// The `note` of every item a response reported, in the order reported.
fn reported_notes(response: &Value) -> Vec<String> {
    response["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["note"]["S"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn a_key_condition_operand_is_held_to_the_types_its_operator_compares() {
    let router = emulator();
    seeded(&router).await;

    // Every datatype `AttributeValue` declares
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`:
    // S N B SS NS BS M L NULL BOOL), as a well-formed value of that type. A comparison
    // orders only S, N and B; `begins_with` only S and B.
    let datatypes: Vec<(&str, Value)> = vec![
        ("S", json!({"S": "a"})),
        ("N", json!({"N": "1"})),
        ("B", json!({"B": "YQ=="})),
        ("SS", json!({"SS": ["a"]})),
        ("NS", json!({"NS": ["1"]})),
        ("BS", json!({"BS": ["YQ=="]})),
        ("M", json!({"M": {"inner": {"S": "a"}}})),
        ("L", json!({"L": [{"S": "a"}]})),
        ("NULL", json!({"NULL": true})),
        ("BOOL", json!({"BOOL": true})),
    ];
    // The comparisons a key condition takes on its sort key, each written over the one
    // operand it compares, beside the operator a refusal about that operand names.
    let comparisons: Vec<(&str, &str)> = vec![
        ("sk = :v", "="),
        ("sk < :v", "<"),
        ("sk <= :v", "<="),
        ("sk > :v", ">"),
        ("sk >= :v", ">="),
        ("sk BETWEEN :v AND :v", "BETWEEN"),
        ("begins_with(sk, :v)", "begins_with"),
    ];
    for (comparison, operator) in &comparisons {
        for (datatype, value) in &datatypes {
            let case = format!("{comparison} over a {datatype} operand");
            let request = json!({
                "TableName": TABLE,
                "KeyConditionExpression": format!("pk = :p AND {comparison}"),
                "ExpressionAttributeValues": {":p": {"S": "p"}, ":v": value},
            });
            // The three key types a comparison orders, and the two a prefix is
            // meaningful for, are what each operator accepts; everything else is the
            // operand-type defect, named with the operator as written.
            let ordered = matches!(*datatype, "S" | "N" | "B");
            let compared = if *operator == "begins_with" {
                matches!(*datatype, "S" | "B")
            } else {
                ordered
            };
            if !compared {
                let stated = refusal(&router, &case, "Query", request).await;
                assert_eq!(
                    stated,
                    incorrect_operand(operator, datatype),
                    "case {case:?}: expected the operand-type refusal, got {stated:?}"
                );
                continue;
            }
            if *datatype != "S" {
                // The operator compares that type, and the sort key the condition names
                // is declared `S`, so the SCHEMA refuses it — after the operator rule
                // passed, which is what orders the two.
                let stated = refusal(&router, &case, "Query", request).await;
                assert_eq!(
                    stated, SCHEMA_TYPE,
                    "case {case:?}: expected the schema-type refusal, got {stated:?}"
                );
                continue;
            }
            let served = ok(&router, "Query", request).await;
            assert!(
                served["Count"].as_i64().is_some(),
                "case {case:?}: an operand of the schema's own type is served, got {served}"
            );
        }
    }

    // The partition key's operand is held to the same rule, and a refusal names the
    // operator the expression wrote there too.
    let case = "a BOOL operand on the partition key";
    let stated = refusal(
        &router,
        case,
        "Query",
        json!({
            "TableName": TABLE,
            "KeyConditionExpression": "pk = :p AND sk = :s",
            "ExpressionAttributeValues": {":p": {"BOOL": false}, ":s": {"S": "a"}},
        }),
    )
    .await;
    assert_eq!(
        stated,
        incorrect_operand("=", "BOOL"),
        "case {case:?}: expected the operand-type refusal, got {stated:?}"
    );
}

#[tokio::test]
async fn a_key_condition_may_write_its_operand_on_either_side() {
    let router = emulator();
    seeded(&router).await;

    // Each row: the comparison with the operand FIRST, and the attribute-first comparison
    // it is the mirror image of. The rows span every comparison that has two sides.
    let mirrored: Vec<(&str, &str)> = vec![
        (":v = sk", "sk = :v"),
        (":v < sk", "sk > :v"),
        (":v <= sk", "sk >= :v"),
        (":v > sk", "sk < :v"),
        (":v >= sk", "sk <= :v"),
    ];
    for (written, mirror) in mirrored {
        let case = format!("{written:?} against {mirror:?}");
        let notes = |condition: &str| {
            let request = json!({
                "TableName": TABLE,
                "KeyConditionExpression": format!("pk = :p AND {condition}"),
                "ExpressionAttributeValues": {":p": {"S": "p"}, ":v": {"S": "b"}},
            });
            async { reported_notes(&ok(&router, "Query", request).await) }
        };
        let operand_first = notes(written).await;
        let attribute_first = notes(mirror).await;
        assert_eq!(
            operand_first, attribute_first,
            "case {case:?}: the two orders select the same items, got {operand_first:?} and \
             {attribute_first:?}"
        );
        assert!(
            !operand_first.is_empty(),
            "case {case:?}: the comparison selects items of the seeded partition, got none"
        );
    }

    // The partition key's equality reads the same way round.
    let served = ok(
        &router,
        "Query",
        json!({
            "TableName": TABLE,
            "KeyConditionExpression": ":p = pk",
            "ExpressionAttributeValues": {":p": {"S": "p"}},
        }),
    )
    .await;
    assert_eq!(
        reported_notes(&served),
        vec!["first", "second", "third", "fourth"],
        "case a value-first partition key: expected the whole partition, got {served}"
    );

    // A refusal about an operand written first names the operator AS WRITTEN rather than
    // the mirror image the comparison states.
    for (condition, operator) in [(":v >= sk", ">="), (":v < sk", "<")] {
        let case = format!("a BOOL operand written first in {condition:?}");
        let stated = refusal(
            &router,
            &case,
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": format!("pk = :p AND {condition}"),
                "ExpressionAttributeValues": {":p": {"S": "p"}, ":v": {"BOOL": true}},
            }),
        )
        .await;
        assert_eq!(
            stated,
            incorrect_operand(operator, "BOOL"),
            "case {case:?}: expected the operator as written, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_defect_a_key_condition_states_first_is_the_one_answered() {
    let router = emulator();
    seeded(&router).await;

    let undefined = "Invalid KeyConditionExpression: An expression attribute value used in \
                     expression is not defined; attribute value: :undef";
    // Each row: a condition carrying TWO defects, and the one its text states first.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "the undefined value is written first",
            "pk = :undef AND begins_with(sk, :n)",
            json!({":n": {"N": "1"}}),
            undefined.to_string(),
        ),
        (
            "the bad operand is written first",
            "begins_with(sk, :n) AND pk = :undef",
            json!({":n": {"N": "1"}}),
            incorrect_operand("begins_with", "N"),
        ),
        (
            "a stray operator between the two operands",
            "pk = = :p",
            json!({":p": {"S": "p"}}),
            "Invalid KeyConditionExpression: Syntax error; token: \"=\", near: \"= = :p\""
                .to_string(),
        ),
        (
            "a stray comma where a comparison begins",
            "pk = :p AND , sk = :s",
            json!({":p": {"S": "p"}, ":s": {"S": "a"}}),
            "Invalid KeyConditionExpression: Syntax error; token: \",\", near: \"AND , sk\""
                .to_string(),
        ),
        (
            "a condition that stops at the end of the expression",
            "pk = :p AND sk =",
            json!({":p": {"S": "p"}}),
            "Invalid KeyConditionExpression: Syntax error; token: \"<EOF>\", near: \"=\""
                .to_string(),
        ),
    ];
    for (case, condition, values, expected) in cases {
        let stated = refusal(
            &router,
            case,
            "Query",
            json!({
                "TableName": TABLE,
                "KeyConditionExpression": condition,
                "ExpressionAttributeValues": values,
            }),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_composite_range_key_is_constrained_from_its_first_attribute() {
    let router = emulator();
    seeded(&router).await;

    // Each row: the condition over the composite index, and the leading range key
    // attributes AWS names beside the attribute whose condition requires them.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a condition on the fourth range key skipping two",
            "gpk = :g AND gsk1 = :one AND gsk4 = :four",
            "gsk2, gsk3",
            "gsk4",
        ),
        (
            "a condition on the third range key skipping one",
            "gpk = :g AND gsk1 = :one AND gsk3 = :three",
            "gsk2",
            "gsk3",
        ),
        (
            "a condition on the second range key with nothing on the first",
            "gpk = :g AND gsk2 = :two",
            "gsk1",
            "gsk2",
        ),
        (
            "a leading range key constrained by a comparison rather than an equality",
            "gpk = :g AND gsk1 > :one AND gsk2 = :two",
            "gsk1",
            "gsk2",
        ),
        (
            "a leading range key constrained by a prefix rather than an equality",
            "gpk = :g AND begins_with(gsk1, :one) AND gsk2 = :two",
            "gsk1",
            "gsk2",
        ),
    ];
    let values = json!({
        ":g": {"S": "g"},
        ":one": {"S": "one"},
        ":two": {"S": "two"},
        ":three": {"N": "3"},
        ":four": {"S": "alpha"},
    });
    for (case, condition, missing, constrained) in cases {
        let used: Value = Value::Object(
            values
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|(placeholder, _)| condition.contains(placeholder.as_str()))
                .collect(),
        );
        let stated = refusal(
            &router,
            case,
            "Query",
            json!({
                "TableName": TABLE,
                "IndexName": COMPOSITE_INDEX,
                "KeyConditionExpression": condition,
                "ExpressionAttributeValues": used,
            }),
        )
        .await;
        let expected = format!(
            "RANGE key attributes {missing} must have equality conditions specified in the \
             query because a condition is present on key attribute {constrained}"
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }

    // Each row: a condition that COVERS the attributes it constrains, and the notes of
    // the index entries it selects — in the index's own order.
    let served: Vec<(&str, &str, Value, Vec<&str>)> = vec![
        (
            // The index orders its entries by the whole key sequence, so the entry whose
            // second range key sorts first comes first.
            "the partition key alone",
            "gpk = :g",
            json!({":g": {"S": "g"}}),
            vec!["third", "first", "second", "fourth"],
        ),
        (
            "an equality on the first range key",
            "gpk = :g AND gsk1 = :one",
            json!({":g": {"S": "g"}, ":one": {"S": "one"}}),
            vec!["third", "first", "second"],
        ),
        (
            "equalities down to the third range key",
            "gpk = :g AND gsk1 = :one AND gsk2 = :two AND gsk3 = :three",
            json!({":g": {"S": "g"}, ":one": {"S": "one"}, ":two": {"S": "two"},
                   ":three": {"N": "3"}}),
            vec!["first", "second"],
        ),
        (
            "a comparison on the last range key behind equalities",
            "gpk = :g AND gsk1 = :one AND gsk2 = :two AND gsk3 = :three AND gsk4 > :alpha",
            json!({":g": {"S": "g"}, ":one": {"S": "one"}, ":two": {"S": "two"},
                   ":three": {"N": "3"}, ":alpha": {"S": "alpha"}}),
            vec!["second"],
        ),
        (
            "a prefix on the last range key behind equalities",
            "gpk = :g AND gsk1 = :one AND gsk2 = :two AND gsk3 = :three AND \
             begins_with(gsk4, :a)",
            json!({":g": {"S": "g"}, ":one": {"S": "one"}, ":two": {"S": "two"},
                   ":three": {"N": "3"}, ":a": {"S": "al"}}),
            vec!["first"],
        ),
    ];
    for (case, condition, values, expected) in served {
        let answered = ok(
            &router,
            "Query",
            json!({
                "TableName": TABLE,
                "IndexName": COMPOSITE_INDEX,
                "KeyConditionExpression": condition,
                "ExpressionAttributeValues": values,
            }),
        )
        .await;
        assert_eq!(
            reported_notes(&answered),
            expected,
            "case {case:?}: expected the entries {expected:?}, got {answered}"
        );
    }
}

#[tokio::test]
async fn a_legacy_key_condition_states_only_an_indexable_comparison() {
    let router = emulator();
    seeded(&router).await;

    // Every comparison the `ComparisonOperator` enum declares
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ComparisonOperator")`:
    // EQ NE IN LE LT GE GT BETWEEN NOT_NULL NULL CONTAINS NOT_CONTAINS BEGINS_WITH), with
    // the operand list that comparison takes and whether a KEY condition may be stated
    // with it: "for KeyConditions, only the following comparison operators are supported:
    // EQ | LE | LT | GE | GT | BEGINS_WITH | BETWEEN" (the `Condition` doc).
    let operators: Vec<(&str, Value, bool)> = vec![
        ("EQ", json!([{"S": "a"}]), true),
        ("LE", json!([{"S": "a"}]), true),
        ("LT", json!([{"S": "b"}]), true),
        ("GE", json!([{"S": "a"}]), true),
        ("GT", json!([{"S": "a"}]), true),
        ("BEGINS_WITH", json!([{"S": "a"}]), true),
        ("BETWEEN", json!([{"S": "a"}, {"S": "c"}]), true),
        ("NE", json!([{"S": "a"}]), false),
        ("IN", json!([{"S": "a"}, {"S": "b"}]), false),
        ("CONTAINS", json!([{"S": "a"}]), false),
        ("NOT_CONTAINS", json!([{"S": "a"}]), false),
        ("NULL", json!([]), false),
        ("NOT_NULL", json!([]), false),
    ];
    let not_indexable = "One or more parameter values were invalid: Attempted conditional \
                         constraint is not an indexable operation";
    for (operator, operands, indexable) in operators {
        let case = format!("the legacy sort-key comparison {operator}");
        let mut condition = json!({"ComparisonOperator": operator});
        if !operands.as_array().cloned().unwrap_or_default().is_empty() {
            condition["AttributeValueList"] = operands;
        }
        let request = json!({
            "TableName": TABLE,
            "KeyConditions": {
                "pk": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "p"}]},
                "sk": condition,
            },
        });
        if indexable {
            let answered = ok(&router, "Query", request).await;
            assert!(
                answered["Count"].as_i64().is_some(),
                "case {case:?}: an indexable comparison is served, got {answered}"
            );
            continue;
        }
        let stated = refusal(&router, &case, "Query", request).await;
        assert_eq!(
            stated, not_indexable,
            "case {case:?}: expected the indexable-operation refusal, got {stated:?}"
        );
    }
}

/// One legacy `Expected` row: the comparison, the attribute it is made on, the value it
/// carries, and whether the stored item satisfies it.
struct ExpectedCase {
    operator: &'static str,
    attribute: &'static str,
    value: Value,
    satisfied: bool,
}

#[tokio::test]
async fn a_legacy_expected_entry_compares_the_value_it_carries() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "LegacyExpected",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "seed the legacy-expected table", "LegacyExpected").await;

    // Each row: one scalar comparison of the `ComparisonOperator` enum, over a stored
    // String or Number attribute, carried in `Value` — the member that states the ONE
    // operand a scalar comparison takes. Both verdicts of each comparison are exercised.
    let cases: Vec<ExpectedCase> = vec![
        ExpectedCase {
            operator: "EQ",
            attribute: "text",
            value: json!({"S": "stored"}),
            satisfied: true,
        },
        ExpectedCase {
            operator: "EQ",
            attribute: "text",
            value: json!({"S": "other"}),
            satisfied: false,
        },
        ExpectedCase {
            operator: "NE",
            attribute: "text",
            value: json!({"S": "other"}),
            satisfied: true,
        },
        ExpectedCase {
            operator: "NE",
            attribute: "text",
            value: json!({"S": "stored"}),
            satisfied: false,
        },
        ExpectedCase {
            operator: "BEGINS_WITH",
            attribute: "text",
            value: json!({"S": "sto"}),
            satisfied: true,
        },
        ExpectedCase {
            operator: "LE",
            attribute: "count",
            value: json!({"N": "5"}),
            satisfied: true,
        },
        ExpectedCase {
            operator: "LT",
            attribute: "count",
            value: json!({"N": "5"}),
            satisfied: false,
        },
        ExpectedCase {
            operator: "GE",
            attribute: "count",
            value: json!({"N": "5"}),
            satisfied: true,
        },
        ExpectedCase {
            operator: "GT",
            attribute: "count",
            value: json!({"N": "4"}),
            satisfied: true,
        },
        ExpectedCase {
            operator: "GT",
            attribute: "count",
            value: json!({"N": "5"}),
            satisfied: false,
        },
    ];
    for case in &cases {
        for operation in ["PutItem", "UpdateItem", "DeleteItem"] {
            let name = format!(
                "{} {} {} through {operation}",
                case.operator, case.attribute, case.value
            );
            // The item every row is read against, written back before each row so the
            // rows do not depend on one another.
            ok(
                &router,
                "PutItem",
                json!({"TableName": "LegacyExpected",
                       "Item": {"pk": {"S": "k"}, "text": {"S": "stored"},
                                "count": {"N": "5"}}}),
            )
            .await;
            let expected = json!({
                case.attribute: {"ComparisonOperator": case.operator, "Value": case.value},
            });
            let request = match operation {
                "PutItem" => json!({"TableName": "LegacyExpected",
                                    "Item": {"pk": {"S": "k"}, "text": {"S": "written"}},
                                    "Expected": expected}),
                "UpdateItem" => json!({"TableName": "LegacyExpected",
                                       "Key": {"pk": {"S": "k"}},
                                       "AttributeUpdates": {
                                           "text": {"Action": "PUT", "Value": {"S": "written"}},
                                       },
                                       "Expected": expected}),
                _ => json!({"TableName": "LegacyExpected", "Key": {"pk": {"S": "k"}},
                            "Expected": expected}),
            };
            let (status, body) = call(&router, operation, request).await;
            if !case.satisfied {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ConditionalCheckFailedException"),
                    "case {name:?}: an unsatisfied comparison refuses the write, got {status} \
                     with {body}"
                );
                continue;
            }
            assert_eq!(
                status, 200,
                "case {name:?}: a satisfied comparison lets the write land, got {status} with \
                 {body}"
            );
            // The write is read back through `GetItem`: a satisfied condition leaves the
            // item the write made, which is what "the write landed" means.
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": "LegacyExpected", "Key": {"pk": {"S": "k"}}}),
            )
            .await;
            if operation == "DeleteItem" {
                assert!(
                    read.get("Item").is_none(),
                    "case {name:?}: the delete landed, so the item is gone, got {read}"
                );
            } else {
                assert_eq!(
                    read["Item"]["text"],
                    json!({"S": "written"}),
                    "case {name:?}: the write landed, so the item carries its value, got {read}"
                );
            }
        }
    }

    // The member pairs an `Expected` entry may not state together are the ones stating
    // the same half of the condition twice, and an operator with nothing to compare names
    // both members it could have been given.
    let conflicts: Vec<(&str, Value, String)> = vec![
        (
            "a value in both members",
            json!({"ComparisonOperator": "EQ", "Value": {"S": "stored"},
                   "AttributeValueList": [{"S": "stored"}]}),
            "Value and AttributeValueList cannot be used together for Attribute: text"
                .to_string(),
        ),
        (
            "an existence test beside a comparison",
            json!({"ComparisonOperator": "EQ", "Exists": true,
                   "AttributeValueList": [{"S": "stored"}]}),
            "Exists and ComparisonOperator cannot be used together for Attribute: text"
                .to_string(),
        ),
        (
            "a comparison with nothing to compare",
            json!({"ComparisonOperator": "EQ"}),
            "Value or AttributeValueList must be used with ComparisonOperator: EQ for \
             Attribute: text"
                .to_string(),
        ),
    ];
    for (case, entry, clause) in conflicts {
        let stated = refusal(
            &router,
            case,
            "PutItem",
            json!({"TableName": "LegacyExpected",
                   "Item": {"pk": {"S": "k"}, "text": {"S": "written"}},
                   "Expected": {"text": entry}}),
        )
        .await;
        let expected = format!(
            "1 validation error detected: One or more parameter values were invalid: {clause}"
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_key_value_is_held_to_the_rules_of_its_own_type_first() {
    let router = emulator();
    seeded(&router).await;

    // Each row: the value a `Key` member carries for the partition key, and the clause it
    // earns. The rows span the three set types and both set rules, beside a set that
    // breaks neither and is simply not the type the schema declares.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a string set listing one member twice",
            json!({"SS": ["a", "a"]}),
            "One or more parameter values were invalid: Input collection [\"a\", \"a\"] \
             contains duplicates.",
        ),
        (
            "a number set listing one member twice, spelled differently",
            json!({"NS": ["1", "1.0"]}),
            "One or more parameter values were invalid: Input collection contains duplicates.",
        ),
        (
            "an empty string set",
            json!({"SS": []}),
            "One or more parameter values were invalid: An string set  may not be empty",
        ),
        (
            "an empty binary set",
            json!({"BS": []}),
            "One or more parameter values were invalid: Binary sets should not be empty",
        ),
        (
            // The collection breaks no rule of its own, so the key is held to the SCHEMA
            // and refused as the key element it is not.
            "a well-formed set of the wrong type for the key",
            json!({"SS": ["a", "b"]}),
            "The provided key element does not match the schema",
        ),
    ];
    for (case, value, clause) in cases {
        // A clause about the COLLECTION a key carried is stated on its own, whichever
        // operation the key arrived on: it refuses the value for what it is rather than
        // reporting a constraint on the member that carried it.
        for operation in ["GetItem", "DeleteItem"] {
            let stated = refusal(
                &router,
                case,
                operation,
                json!({"TableName": TABLE,
                       "Key": {"pk": value.clone(), "sk": {"S": "a"}}}),
            )
            .await;
            assert_eq!(
                stated, clause,
                "case {case:?} through {operation}: expected {clause:?}, got {stated:?}"
            );
        }
    }

    // A BINARY set names its type in the clause and echoes its members as the base64 the
    // request spelled them in, whichever order and however many times it listed them: the
    // rows below differ along both, so the assertions are on the clause the rule states
    // and on each member being echoed rather than on the text of one collection.
    let binary: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a binary set listing one member twice",
            json!({"BS": ["YWJj", "YWJj"]}),
            vec!["YWJj"],
        ),
        (
            "a binary set whose repeated member is not the first, out of byte order",
            json!({"BS": ["eg==", "AQ==", "AQ=="]}),
            vec!["eg==", "AQ=="],
        ),
    ];
    for (case, value, members) in binary {
        let stated = refusal(
            &router,
            case,
            "DeleteItem",
            json!({"TableName": TABLE, "Key": {"pk": value, "sk": {"S": "a"}}}),
        )
        .await;
        let opening = "One or more parameter values were invalid: Input collection [";
        assert!(
            stated.starts_with(opening) && stated.ends_with("]of type BS contains duplicates."),
            "case {case:?}: expected the binary duplicate-collection clause, got {stated:?}"
        );
        for member in members {
            assert!(
                stated.contains(&format!("\"{member}\"")),
                "case {case:?}: expected the clause to echo {member:?}, got {stated:?}"
            );
        }
    }
}
