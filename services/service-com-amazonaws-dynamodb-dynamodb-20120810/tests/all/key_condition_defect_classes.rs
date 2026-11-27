// SPDX-License-Identifier: CC0-1.0

//! WHICH CLASS a `KeyConditionExpression` defect belongs to, driven end to end.
//!
//! RULE under test: a key condition constrains the KEY ATTRIBUTES of the schema it reads,
//! one clause per attribute, through the comparisons that member takes — so each way a
//! condition breaks that is its own class, named by its own sentence:
//!
//! * a condition on a DOCUMENT PATH addresses nothing a key condition can speak about (a
//!   key attribute is a top-level attribute), and a name that is one of DynamoDB's
//!   RESERVED words is refused for that FIRST, because the expression has to reach that
//!   attribute through `ExpressionAttributeNames` whatever it writes after it;
//! * two clauses about ONE attribute break the member's one-clause rule, whichever half of
//!   the key they are about and however many clauses the condition states in all;
//! * an operand its comparison cannot be made against AT ALL — a collection, or a
//!   non-orderable value under a comparator that ORDERS — is its own sentence, naming the
//!   operator as the expression wrote it, while an EQUALITY operand the comparison DOES take
//!   is held to the type the SCHEMA declares for the key it binds to. The types a comparison
//!   ORDERS and the prefix `begins_with` takes are that operator's own vocabulary, settled by
//!   the request, so that sentence is answered whatever table was named; what EQUALITY's
//!   operand may be is the key schema's business, so there the existence check decides first.
//!
//! The CONTROL at the end drives the forms the member DOES take, including the
//! operand-first spelling of a comparison, so the refusals above are this classification
//! and not a blanket rejection.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table the rows read: a string partition key and a string sort key.
const TABLE: &str = "KeyConditionDefectClasses";

/// A table name no row creates, which is what makes the existence check decide.
const ABSENT: &str = "NoSuchKeyConditionTable";

/// AWS's fixed sentence for a condition on a path that reaches inside an attribute.
const NESTED: &str = "Invalid KeyConditionExpression: KeyConditionExpressions cannot have \
                      conditions on nested attributes";

/// AWS's fixed sentence for a condition that states two clauses about one key attribute.
const ONE_PER_KEY: &str = "Invalid KeyConditionExpression: KeyConditionExpressions must only \
                           contain one condition per key";

/// AWS's fixed sentence for an operand whose type is not the one the SCHEMA declares for
/// the key attribute it is compared with.
const SCHEMA_TYPE: &str = "One or more parameter values were invalid: Condition parameter type \
                           does not match schema type";

/// AWS's clause for an operand its comparison or function cannot be made against at all.
fn operand_type(operator_or_function: &str, datatype: &str) -> String {
    format!(
        "Invalid KeyConditionExpression: Incorrect operand type for operator or function; \
         operator or function: {operator_or_function}, operand type: {datatype}"
    )
}

/// AWS's clause for a reserved word written out as an attribute name.
fn reserved(word: &str) -> String {
    format!(
        "Invalid KeyConditionExpression: Attribute name is a reserved keyword; reserved \
         keyword: {word}"
    )
}

/// The emulator with the table and two items in place, so a served row has something to
/// select and no row is refused for its table instead of for the rule under test.
async fn seeded() -> MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S"), ("sk", "RANGE", "S")]).await;
    for sort in ["a", "b", "c"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE,
                   "Item": {"pk": {"S": "p"}, "sk": {"S": sort}, "note": {"S": sort}}}),
        )
        .await;
    }
    router
}

/// The `Query` a row sends, against `table`.
fn query(table: &str, condition: &str, values: Value) -> Value {
    json!({"TableName": table,
           "KeyConditionExpression": condition,
           "ExpressionAttributeValues": values})
}

/// The one refusal a row earns, asserted to be a `ValidationException` first.
async fn refusal(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let (status, answered) = call(router, "Query", request).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
    );
    message(&answered)
}

#[tokio::test]
async fn a_condition_on_a_document_path_is_refused_as_a_nested_attribute() {
    let router = seeded().await;

    // The rows differ along the FORM the path takes (a dotted step, a list index, a step
    // behind an alias), which half of the key it is written about, and where in the
    // condition it sits.
    for (case, condition, names) in [
        ("a dotted step on the partition key", "pk.foo = :p", json!({})),
        ("a list index on the partition key", "pk[0] = :p", json!({})),
        (
            "a dotted step on the sort key, behind the partition clause",
            "pk = :p AND sk.foo = :s",
            json!({}),
        ),
        (
            "a dotted step behind an alias",
            "pk = :p AND #s.foo = :s",
            json!({"#s": "sk"}),
        ),
        (
            "a list index behind an alias",
            "pk = :p AND #s[1] = :s",
            json!({"#s": "sk"}),
        ),
    ] {
        let mut request = query(
            TABLE,
            condition,
            json!({":p": {"S": "p"}, ":s": {"S": "a"}}),
        );
        if names != json!({}) {
            request["ExpressionAttributeNames"] = names;
        }
        let stated = refusal(&router, case, request).await;
        assert_eq!(
            stated, NESTED,
            "case {case:?}: expected {NESTED:?}, got {stated:?}"
        );
    }

    // A RESERVED word at the head of such a path is refused for the reserved word first:
    // the caller has to reach that attribute through `ExpressionAttributeNames` whatever
    // it writes after it. The rows use two different reserved words, matched without
    // regard to case the way the expression keywords are.
    for (case, condition, word) in [
        ("a reserved head, lower case", "data.foo = :p", "data"),
        ("a reserved head, upper case", "VALUE.foo = :p", "VALUE"),
    ] {
        let stated = refusal(
            &router,
            case,
            query(TABLE, condition, json!({":p": {"S": "p"}})),
        )
        .await;
        assert_eq!(
            stated,
            reserved(word),
            "case {case:?}: expected {:?}, got {stated:?}",
            reserved(word)
        );
    }
}

#[tokio::test]
async fn two_clauses_about_one_key_attribute_break_the_one_clause_rule() {
    let router = seeded().await;

    // The rows differ along which half of the key is doubled, how many clauses the
    // condition states in all (two, three — more than the schema has key attributes), the
    // comparisons the two clauses use, and whether the second clause is written behind an
    // alias or parenthesised.
    for (case, condition, names, values) in [
        (
            "the sort key constrained twice",
            "pk = :p AND sk = :s AND sk = :t",
            json!({}),
            json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":t": {"S": "b"}}),
        ),
        (
            "the sort key constrained twice with different comparisons",
            "pk = :p AND sk > :s AND sk < :t",
            json!({}),
            json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":t": {"S": "b"}}),
        ),
        (
            "the partition key constrained twice",
            "pk = :p AND pk = :t",
            json!({}),
            json!({":p": {"S": "p"}, ":t": {"S": "q"}}),
        ),
        (
            "the sort key doubled behind an alias",
            "pk = :p AND sk = :s AND #s = :t",
            json!({"#s": "sk"}),
            json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":t": {"S": "b"}}),
        ),
        (
            "the sort key doubled inside a group",
            "pk = :p AND (sk = :s AND sk = :t)",
            json!({}),
            json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":t": {"S": "b"}}),
        ),
    ] {
        let mut request = query(TABLE, condition, values);
        if names != json!({}) {
            request["ExpressionAttributeNames"] = names;
        }
        let stated = refusal(&router, case, request).await;
        assert_eq!(
            stated, ONE_PER_KEY,
            "case {case:?}: expected {ONE_PER_KEY:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_operand_is_held_to_its_comparison_first_and_to_the_schema_second() {
    let router = seeded().await;

    // Each row: the condition, the operand it compares the sort key with, the class
    // the refusal belongs to — the COMPARISON's own domain, or the type the SCHEMA declares
    // for the key — and whether that class is settled by the REQUEST alone. The rows span the
    // comparators that order, the one function the member has, every collection type and the
    // two single-valued types that are no key type.
    for (case, condition, value, outside_the_comparison, settled_by_the_request) in [
        (
            "a list under an ordering comparator",
            "pk = :p AND sk >= :v",
            json!({"L": [{"S": "a"}]}),
            Some(operand_type(">=", "L")),
            true,
        ),
        (
            "a map under an ordering comparator",
            "pk = :p AND sk < :v",
            json!({"M": {"inner": {"S": "a"}}}),
            Some(operand_type("<", "M")),
            true,
        ),
        (
            "a boolean under an ordering comparator",
            "pk = :p AND sk > :v",
            json!({"BOOL": true}),
            Some(operand_type(">", "BOOL")),
            true,
        ),
        (
            "a null under an ordering comparator",
            "pk = :p AND sk <= :v",
            json!({"NULL": true}),
            Some(operand_type("<=", "NULL")),
            true,
        ),
        (
            "a string set under equality",
            "pk = :p AND sk = :v",
            json!({"SS": ["a"]}),
            Some(operand_type("=", "SS")),
            false,
        ),
        (
            "a number set under equality",
            "pk = :p AND sk = :v",
            json!({"NS": ["1"]}),
            Some(operand_type("=", "NS")),
            false,
        ),
        (
            "a number where the function needs a prefix",
            "pk = :p AND begins_with(sk, :v)",
            json!({"N": "1"}),
            Some(operand_type("begins_with", "N")),
            true,
        ),
        (
            "a boolean under equality",
            "pk = :p AND sk = :v",
            json!({"BOOL": true}),
            None,
            false,
        ),
        (
            "a null under equality",
            "pk = :p AND sk = :v",
            json!({"NULL": true}),
            None,
            false,
        ),
        (
            "a number under equality, against a string sort key",
            "pk = :p AND sk = :v",
            json!({"N": "1"}),
            None,
            false,
        ),
    ] {
        let request = query(
            TABLE,
            condition,
            json!({":p": {"S": "p"}, ":v": value.clone()}),
        );
        let stated = refusal(&router, case, request).await;
        let expected = outside_the_comparison
            .clone()
            .unwrap_or(SCHEMA_TYPE.to_string());
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );

        // The SAME condition against a table this account does not hold: the vocabulary a
        // comparison or function has of its OWN is settled by the request, so that class is
        // answered whatever table was named, while what EQUALITY's operand may be is the key
        // SCHEMA's business — there the existence check decides.
        let (status, answered) = call(
            &router,
            "Query",
            query(ABSENT, condition, json!({":p": {"S": "p"}, ":v": value})),
        )
        .await;
        let (code, stated) = if settled_by_the_request {
            ("ValidationException".to_string(), expected.clone())
        } else {
            (
                "ResourceNotFoundException".to_string(),
                "Requested resource not found".to_string(),
            )
        };
        assert_eq!(
            (status, error_code(&answered), message(&answered)),
            (400, code.clone(), stated.clone()),
            "case {case:?} on the missing table: expected {code} with {stated:?}, got \
             {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn the_forms_the_member_takes_are_served() {
    let router = seeded().await;

    // The CONTROL: each form `Query$KeyConditionExpression` spells, including the spelling
    // that writes the OPERAND first, beside the sort keys it must select from the three
    // stored items.
    for (case, condition, values, selected) in [
        (
            "the partition key alone",
            "pk = :p",
            json!({":p": {"S": "p"}}),
            vec!["a", "b", "c"],
        ),
        (
            "a sort key equality",
            "pk = :p AND sk = :s",
            json!({":p": {"S": "p"}, ":s": {"S": "b"}}),
            vec!["b"],
        ),
        (
            "a sort key comparison",
            "pk = :p AND sk > :s",
            json!({":p": {"S": "p"}, ":s": {"S": "a"}}),
            vec!["b", "c"],
        ),
        (
            "the same comparison with the operand written first",
            "pk = :p AND :s < sk",
            json!({":p": {"S": "p"}, ":s": {"S": "a"}}),
            vec!["b", "c"],
        ),
        (
            "an operand-first comparison that orders the other way",
            "pk = :p AND :s >= sk",
            json!({":p": {"S": "p"}, ":s": {"S": "b"}}),
            vec!["a", "b"],
        ),
        (
            "a range over the sort key",
            "pk = :p AND sk BETWEEN :lo AND :hi",
            json!({":p": {"S": "p"}, ":lo": {"S": "a"}, ":hi": {"S": "b"}}),
            vec!["a", "b"],
        ),
        (
            "a prefix on the sort key",
            "pk = :p AND begins_with(sk, :s)",
            json!({":p": {"S": "p"}, ":s": {"S": "c"}}),
            vec!["c"],
        ),
    ] {
        let answered = ok(&router, "Query", query(TABLE, condition, values)).await;
        let notes: Vec<String> = answered["Items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|item| item["sk"]["S"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            notes, selected,
            "case {case:?}: expected the sort keys {selected:?}, got {notes:?} from {answered}"
        );
    }
}
