// SPDX-License-Identifier: CC0-1.0

//! WHICH rule a `KeyConditionExpression` whose operands break several of them is refused
//! for, driven end to end.
//!
//! THE RULE: the defects the OPERANDS of a key condition carry are answered in ONE reading,
//! in the order the expression WROTE them, and the first of them is the refusal. Within one
//! comparison that order puts the rule about an operand ON ITS OWN — a value of a type the
//! operator or function it was written with does not compare, refused as the operand-type
//! defect naming the operator as written and that value's `AttributeValue` type tag — ahead of
//! every rule relating two operands to EACH OTHER: a bound no comparison orders is no bound of
//! a range to begin with, so the two `BETWEEN` rules about the pair (one type across both
//! bounds, the lower one not above the upper) are asked only once each bound is a value the
//! comparator orders. Across clauses the same order decides between an ill-typed operand and a
//! `:placeholder` the request never defined: whichever the text states first.
//!
//! WHEN each is answered still follows what settles it: a token nothing defines, a pair of
//! literal bounds and an operand outside the vocabulary a comparison or function has of its own
//! are facts of the REQUEST, answered before the addressed table is resolved, while what
//! EQUALITY's operand may be is the key SCHEMA's business, read where the condition binds to it
//! (`crate::absent_table_before_key_binding`) — and where there is no schema, the
//! request-settled defect the text states after it answers instead.
//!
//! The same ordering is what the condition and filter grammar of this service has always
//! applied to a `BETWEEN` it reads, so a range written in a `FilterExpression` and one written
//! in a `KeyConditionExpression` are refused for the same rule.
//!
//! The rows span what the rule generalises over: which `AttributeValue` type the offending
//! bound has (every type `AttributeValue` declares that no comparison orders), which bound
//! carries it (lower, upper, both), the two pairwise rules that apply once both bounds are
//! ordered (two types, and the wrong way round) over each ordered type, which member the
//! `BETWEEN` is written in, and whether an undefined token is written before or after the
//! ill-typed operand.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table every row reads: a composite primary key whose two attributes are Strings.
const TABLE: &str = "KeyConditionOperandPrecedence";

/// The partition key value the seeded item is held under.
const PARTITION: &str = "p";

/// The sort key value of the seeded item, which the served rows select.
const SORT: &str = "alpha";

/// AWS's sentence for an operand whose type the operator or function does not compare, with
/// the operator as the expression WROTE it and the operand's `AttributeValue` type.
fn incorrect_operand(member: &str, operator: &str, datatype: &str) -> String {
    format!(
        "Invalid {member}: Incorrect operand type for operator or function; operator or \
         function: {operator}, operand type: {datatype}"
    )
}

/// One attribute value as a refusal about an OPERAND echoes it: its type tag and the text it
/// carries.
fn rendered(datatype: &str, text: &str) -> String {
    format!("AttributeValue: {{{datatype}:{text}}}")
}

/// AWS's sentence for a `BETWEEN` whose two bounds are of different types.
fn different_types(member: &str, low: (&str, &str), high: (&str, &str)) -> String {
    format!(
        "Invalid {member}: The BETWEEN operator requires same data type for lower and upper \
         bounds; lower bound operand: {}, upper bound operand: {}",
        rendered(low.0, low.1),
        rendered(high.0, high.1)
    )
}

/// AWS's sentence for a `BETWEEN` whose bounds are the wrong way round.
fn bounds_out_of_order(member: &str, low: (&str, &str), high: (&str, &str)) -> String {
    format!(
        "Invalid {member}: The BETWEEN operator requires upper bound to be greater than or \
         equal to lower bound; lower bound operand: {}, upper bound operand: {}",
        rendered(low.0, low.1),
        rendered(high.0, high.1)
    )
}

/// AWS's sentence for a `:placeholder` `ExpressionAttributeValues` does not define.
fn undefined_value(member: &str, placeholder: &str) -> String {
    format!(
        "Invalid {member}: An expression attribute value used in expression is not defined; \
         attribute value: {placeholder}"
    )
}

/// Create [`TABLE`], wait out the documented `CREATING` -> `ACTIVE` transition and seed the
/// one item the served rows select.
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
                {"AttributeName": "sk", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "seed the operand-precedence table", TABLE).await;
    ok(
        &router,
        "PutItem",
        json!({
            "TableName": TABLE,
            "Item": {"pk": {"S": PARTITION}, "sk": {"S": SORT}, "n": {"N": "5"}},
        }),
    )
    .await;
    router
}

/// The one refusal a `Query` earns, asserted to be a `ValidationException` first so a row
/// answered with another class says so.
async fn refusal(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let (status, body) = call(router, "Query", request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
    );
    message(&body)
}

/// The `Query` that states `condition` over `values` against the seeded table.
fn query(condition: &str, values: Value) -> Value {
    json!({
        "TableName": TABLE,
        "KeyConditionExpression": condition,
        "ExpressionAttributeValues": values,
    })
}

/// Every type `AttributeValue` declares that NO comparison orders, as a well-formed value of
/// that type.
///
/// Source: this service's Smithy model, the members of the `AttributeValue` union
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`: S N B SS NS
/// BS M L NULL BOOL) less the three a comparison orders ("For type Number, value comparisons
/// are numeric. String value comparisons ... are based on ASCII character code values. For
/// Binary, DynamoDB treats each byte ... as unsigned" — the `AttributeValueList` doc).
fn unordered_types() -> Vec<(&'static str, Value)> {
    vec![
        ("BOOL", json!({"BOOL": false})),
        ("NULL", json!({"NULL": true})),
        ("L", json!({"L": [{"S": "a"}]})),
        ("M", json!({"M": {"inner": {"S": "a"}}})),
        ("SS", json!({"SS": ["a", "b"]})),
        ("NS", json!({"NS": ["1", "2"]})),
        ("BS", json!({"BS": ["YQ=="]})),
    ]
}

/// The three types a comparison ORDERS, as a well-formed value of each and as the text a
/// refusal echoes for it. `B` is echoed base64-encoded, which is how the wire carries it.
fn ordered_types() -> [(&'static str, Value, &'static str); 3] {
    [
        ("S", json!({"S": "m"}), "m"),
        ("N", json!({"N": "7"}), "7"),
        ("B", json!({"B": "YQ=="}), "YQ=="),
    ]
}

/// RULE: a `BETWEEN` bound of a type no comparison orders is refused as the OPERAND it is —
/// with `BETWEEN` as the operator and that bound's type — whichever of the two bounds carries
/// it and whatever the other bound is, because the rules relating the pair are asked only once
/// each bound is a value the comparator orders.
#[tokio::test]
async fn a_between_bound_no_comparison_orders_is_refused_as_that_operand() {
    let router = seeded().await;
    for (datatype, value) in unordered_types() {
        // Each row: where the offending bound sits, the condition, and the values it binds.
        // The other bound is a well-formed String, which is the type the sort key declares,
        // so nothing but the offending bound is wrong with the range.
        let rows: Vec<(String, &str, Value)> = vec![
            (
                format!("the LOWER bound is a {datatype} beside a String upper bound"),
                "pk = :p AND sk BETWEEN :lo AND :hi",
                json!({":p": {"S": PARTITION}, ":lo": value.clone(), ":hi": {"S": "z"}}),
            ),
            (
                format!("the UPPER bound is a {datatype} beside a String lower bound"),
                "pk = :p AND sk BETWEEN :lo AND :hi",
                json!({":p": {"S": PARTITION}, ":lo": {"S": "a"}, ":hi": value.clone()}),
            ),
            (
                format!("BOTH bounds are {datatype} values"),
                "pk = :p AND sk BETWEEN :lo AND :hi",
                json!({":p": {"S": PARTITION}, ":lo": value.clone(), ":hi": value.clone()}),
            ),
            (
                format!("a {datatype} lower bound in a condition the sort key opens"),
                "sk BETWEEN :lo AND :hi AND pk = :p",
                json!({":p": {"S": PARTITION}, ":lo": value.clone(), ":hi": {"S": "z"}}),
            ),
        ];
        for (case, condition, values) in rows {
            let stated = refusal(&router, &case, query(condition, values)).await;
            assert_eq!(
                stated,
                incorrect_operand("KeyConditionExpression", "BETWEEN", datatype),
                "case {case:?}: expected the operand-type refusal, got {stated:?}"
            );
        }
    }
}

/// RULE: once BOTH bounds are values the comparator orders, the rules about the PAIR apply —
/// two bounds of different types draw no range, and a lower bound above the upper one draws
/// none either — each stated over the two operands as written.
#[tokio::test]
async fn two_ordered_bounds_are_then_held_to_the_rules_about_the_pair() {
    let router = seeded().await;

    // Every ordered PAIR of the three types a comparison orders: the rule is about the two
    // bounds differing, whichever two types they are and whichever way round.
    for (low_type, low_value, low_text) in ordered_types() {
        for (high_type, high_value, high_text) in ordered_types() {
            if low_type == high_type {
                continue;
            }
            let case = format!("a BETWEEN over a {low_type} and a {high_type} bound");
            let stated = refusal(
                &router,
                &case,
                query(
                    "pk = :p AND sk BETWEEN :lo AND :hi",
                    json!({":p": {"S": PARTITION}, ":lo": low_value, ":hi": high_value}),
                ),
            )
            .await;
            assert_eq!(
                stated,
                different_types(
                    "KeyConditionExpression",
                    (low_type, low_text),
                    (high_type, high_text)
                ),
                "case {case:?}: expected the two-types refusal, got {stated:?}"
            );
        }
    }

    // One type across both bounds, stated the wrong way round: the comparator's own rule.
    for (case, low, high) in [
        ("two strings the wrong way round", ("S", "z"), ("S", "a")),
        ("two numbers the wrong way round", ("N", "9"), ("N", "1")),
    ] {
        let (low_value, high_value) = (value_of(low), value_of(high));
        let stated = refusal(
            &router,
            case,
            query(
                "pk = :p AND sk BETWEEN :lo AND :hi",
                json!({":p": {"S": PARTITION}, ":lo": low_value, ":hi": high_value}),
            ),
        )
        .await;
        assert_eq!(
            stated,
            bounds_out_of_order("KeyConditionExpression", low, high),
            "case {case:?}: expected the bound-ORDER refusal, got {stated:?}"
        );
    }
}

/// One `AttributeValue` on the wire, from its type tag and the text it carries.
fn value_of(typed: (&str, &str)) -> Value {
    let (datatype, text) = typed;
    json!({datatype: text})
}

/// RULE: the same ordering holds for a `BETWEEN` written in the condition and filter grammar,
/// so one range is refused for the same rule whichever member carried it.
#[tokio::test]
async fn a_filter_expressions_between_is_held_to_the_same_order_of_rules() {
    let router = seeded().await;

    for (datatype, value) in unordered_types() {
        let case = format!("a filter BETWEEN whose lower bound is a {datatype}");
        let (status, body) = call(
            &router,
            "Scan",
            json!({
                "TableName": TABLE,
                "FilterExpression": "n BETWEEN :lo AND :hi",
                "ExpressionAttributeValues": {":lo": value, ":hi": {"N": "9"}},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            message(&body),
            incorrect_operand("FilterExpression", "BETWEEN", datatype),
            "case {case:?}: expected the operand-type refusal, got {:?}",
            message(&body)
        );
    }
}

/// RULE: across the clauses of one condition the same order decides — the defect the
/// expression's text states FIRST is the refusal, whether that is an ill-typed operand or a
/// `:placeholder` the request never defined.
#[tokio::test]
async fn the_operand_defect_written_first_is_the_one_answered() {
    let router = seeded().await;

    // Each row: the condition, the values it binds, and the message the rule requires. Each
    // carries BOTH defects, and they differ only in which of the two is written first.
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a bad prefix operand before an undefined token",
            "begins_with(sk, :n) AND pk = :undef",
            json!({":n": {"N": "1"}}),
            incorrect_operand("KeyConditionExpression", "begins_with", "N"),
        ),
        (
            "an undefined token before a bad prefix operand",
            "pk = :undef AND begins_with(sk, :n)",
            json!({":n": {"N": "1"}}),
            undefined_value("KeyConditionExpression", ":undef"),
        ),
        (
            "a bad BETWEEN bound before an undefined upper bound",
            "pk = :p AND sk BETWEEN :b AND :undef",
            json!({":p": {"S": PARTITION}, ":b": {"BOOL": false}}),
            incorrect_operand("KeyConditionExpression", "BETWEEN", "BOOL"),
        ),
        (
            "an undefined lower bound before a bad upper bound",
            "pk = :p AND sk BETWEEN :undef AND :b",
            json!({":p": {"S": PARTITION}, ":b": {"BOOL": false}}),
            undefined_value("KeyConditionExpression", ":undef"),
        ),
        (
            "a bad operand written OPERAND first, before an undefined token",
            ":l >= sk AND pk = :undef",
            json!({":l": {"L": [{"S": "a"}]}}),
            incorrect_operand("KeyConditionExpression", ">=", "L"),
        ),
        (
            "a bad operand in the middle clause of three",
            "pk = :p AND begins_with(sk, :n) AND extra = :undef",
            json!({":p": {"S": PARTITION}, ":n": {"N": "7"}}),
            incorrect_operand("KeyConditionExpression", "begins_with", "N"),
        ),
    ];
    for (case, condition, values, expected) in rows {
        let stated = refusal(&router, case, query(condition, values)).await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE's other half: a refusal is reported only where the service can judge it. A condition
/// whose first defect is one only the key schema settles, addressed to a table this account
/// does not hold, earns the defect the REQUEST settles after it — and the absent resource where
/// its text states none.
#[tokio::test]
async fn an_absent_table_answers_only_where_no_request_settled_defect_is_written() {
    let router = emulator();
    // A table name nothing here creates, which is what makes the lookup decide.
    let absent = "NoSuchTableForOperandPrecedence";

    // Each row: the condition, the values it binds, the error code it earns and its message.
    let rows: Vec<(&str, &str, Value, &str, String)> = vec![
        (
            // The prefix a `begins_with` takes is that function's own vocabulary, so the
            // defect is the REQUEST's and is answered whatever table was named — and its text
            // states it before the undefined token, so it is the one answered.
            "a bad prefix operand first, an undefined token after it",
            "begins_with(sk, :n) AND pk = :undef",
            json!({":n": {"N": "1"}}),
            "ValidationException",
            incorrect_operand("KeyConditionExpression", "begins_with", "N"),
        ),
        (
            "an undefined token first, a bad prefix operand after it",
            "pk = :undef AND begins_with(sk, :n)",
            json!({":n": {"N": "1"}}),
            "ValidationException",
            undefined_value("KeyConditionExpression", ":undef"),
        ),
        (
            "an undefined token and nothing else",
            "pk = :undef",
            json!({}),
            "ValidationException",
            undefined_value("KeyConditionExpression", ":undef"),
        ),
        (
            "a bad prefix operand and every token defined",
            "pk = :p AND begins_with(sk, :n)",
            json!({":p": {"S": PARTITION}, ":n": {"N": "1"}}),
            "ValidationException",
            incorrect_operand("KeyConditionExpression", "begins_with", "N"),
        ),
        (
            // The pair rules about one `BETWEEN`'s bounds are the REQUEST's own too — two
            // literals draw their range, or fail to, without any schema — so the bound the
            // comparator cannot order is answered first and the pair where every bound is
            // ordered.
            "a bad BETWEEN bound and every token defined",
            "pk = :p AND sk BETWEEN :b AND :hi",
            json!({":p": {"S": PARTITION}, ":b": {"BOOL": false}, ":hi": {"S": "z"}}),
            "ValidationException",
            incorrect_operand("KeyConditionExpression", "BETWEEN", "BOOL"),
        ),
        (
            "two BETWEEN bounds of different ordered types",
            "pk = :p AND sk BETWEEN :lo AND :hi",
            json!({":p": {"S": PARTITION}, ":lo": {"S": "m"}, ":hi": {"N": "7"}}),
            "ValidationException",
            different_types("KeyConditionExpression", ("S", "m"), ("N", "7")),
        ),
        (
            // What EQUALITY's operand may be is the key SCHEMA's business, so that defect is
            // passed over where there is no schema: the absent resource answers.
            "an equality operand no key can be, and every token defined",
            "pk = :m AND sk = :s",
            json!({":m": {"M": {"inner": {"S": "a"}}}, ":s": {"S": "a"}}),
            "ResourceNotFoundException",
            "Requested resource not found".to_string(),
        ),
        (
            // And the undefined token its text states after that defect is what answers when
            // the condition carries one.
            "an equality operand no key can be, an undefined token after it",
            "pk = :m AND sk = :undef",
            json!({":m": {"M": {"inner": {"S": "a"}}}}),
            "ValidationException",
            undefined_value("KeyConditionExpression", ":undef"),
        ),
    ];
    for (case, condition, values, code, expected) in rows {
        let (status, body) = call(
            &router,
            "Query",
            json!({
                "TableName": absent,
                "KeyConditionExpression": condition,
                "ExpressionAttributeValues": values,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (400, code.to_string(), expected.clone()),
            "case {case:?}: expected {code} with {expected:?}, got {status} with {body}"
        );
    }
}

/// RULE's boundary: a `BETWEEN` whose bounds are both of the type the sort key declares and
/// in the comparator's own order is SERVED, so no row above passes because ranges are refused
/// in general.
#[tokio::test]
async fn a_well_formed_range_over_the_sort_key_is_served() {
    let router = seeded().await;
    for (case, low, high, selects) in [
        ("a range the stored sort key falls inside", "a", "z", 1),
        ("a range the stored sort key falls outside", "x", "z", 0),
        ("a range whose bounds are equal to each other", SORT, SORT, 1),
    ] {
        let answered = ok(
            &router,
            "Query",
            query(
                "pk = :p AND sk BETWEEN :lo AND :hi",
                json!({":p": {"S": PARTITION}, ":lo": {"S": low}, ":hi": {"S": high}}),
            ),
        )
        .await;
        assert_eq!(
            answered["Count"],
            json!(selects),
            "case {case:?}: expected {selects} item(s), got {answered}"
        );
    }
}
