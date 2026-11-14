// SPDX-License-Identifier: CC0-1.0

//! WHEN the table a read is addressed to is resolved, relative to the defects its key
//! condition can carry.
//!
//! THE RULE: a `KeyConditionExpression` defect that the REQUEST alone settles — the
//! expression's syntax, and a substitution token the expression uses but the request never
//! defined — is refused before the addressed table is resolved, so it is answered even for a
//! table this account does not hold. Every other key-condition rule is about how the
//! condition BINDS to the key attributes of the schema it is read against — how many
//! comparisons that schema has key attributes for, and what `AttributeValue` type each
//! operand may be for the comparison it is written in — so it is only reached once the table
//! has been found, and a `Query` naming an absent table is refused
//! `ResourceNotFoundException` ("the operation tried to access a nonexistent table or
//! index") whatever its operands and however many clauses it states.
//!
//! The rows span what the rule generalises over: which KIND of defect the condition carries
//! (syntax, an undefined value placeholder, an undefined name alias, an operand of a type no
//! comparison orders, an operand of a type `begins_with` has no prefix for, more clauses than
//! the schema has key attributes, and the combinations of a binding defect with another
//! binding defect), whether the table is absent or present, and whether the read is a `Query`
//! on the table itself or on one of its secondary indexes.
//!
//! The same rows against a table that IS there pin the other half: the binding refusals are
//! still stated, so nothing here is "absent tables answer everything".

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};

/// The table the present-table rows read: a composite primary key whose sort key is a
/// String.
const TABLE: &str = "KeyBindingOrder";

/// A table name no request here creates, which is what makes the existence check decide.
const ABSENT: &str = "NeverCreatedTable";

/// AWS's fixed data-plane sentence for a table (or index) that is not there, with no
/// resource detail appended (`Query`'s `ResourceNotFoundException`).
const NOT_FOUND: &str = "Requested resource not found";

/// AWS's clause for an operand whose type the operator or function does not compare, with
/// the operator as the expression wrote it and the operand's datatype.
fn incorrect_operand(operator: &str, datatype: &str) -> String {
    format!(
        "Invalid KeyConditionExpression: Incorrect operand type for operator or function; \
         operator or function: {operator}, operand type: {datatype}"
    )
}

/// Create [`TABLE`] and wait out the documented `CREATING` -> `ACTIVE` transition.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
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
    describe_when_active(&router, "seed the key-binding table", TABLE).await;
    router
}

/// Each row: the case, the `KeyConditionExpression`, its `ExpressionAttributeValues`, and
/// what the rule requires of a request carrying it — either the defect is the REQUEST's and
/// is stated whatever table was addressed, or it is a BINDING defect and an absent table
/// answers first.
struct Row {
    case: &'static str,
    condition: &'static str,
    values: Value,
    /// `Some(message)` when the defect precedes the table lookup, `None` when the table's
    /// existence decides.
    before_the_lookup: Option<String>,
    /// What the same request earns once the table IS there.
    against_the_table: String,
}

fn rows() -> Vec<Row> {
    let undefined_value = "Invalid KeyConditionExpression: An expression attribute value used \
                           in expression is not defined; attribute value: :undef"
        .to_string();
    let undefined_name = "Invalid KeyConditionExpression: An expression attribute name used in \
                          the document path is not defined; attribute name: #undef"
        .to_string();
    vec![
        Row {
            case: "an undefined value placeholder",
            condition: "pk = :undef",
            values: json!({}),
            before_the_lookup: Some(undefined_value.clone()),
            against_the_table: undefined_value,
        },
        Row {
            case: "an undefined name alias",
            condition: "#undef = :p",
            values: json!({":p": {"S": "p"}}),
            before_the_lookup: Some(undefined_name.clone()),
            against_the_table: undefined_name,
        },
        Row {
            case: "a syntax error",
            condition: "pk = = :p",
            values: json!({":p": {"S": "p"}}),
            before_the_lookup: Some(
                "Invalid KeyConditionExpression: Syntax error; token: \"=\", near: \"= = :p\""
                    .to_string(),
            ),
            against_the_table: "Invalid KeyConditionExpression: Syntax error; token: \"=\", \
                                near: \"= = :p\""
                .to_string(),
        },
        Row {
            case: "an operand of a type no comparison orders",
            condition: "pk = :p",
            values: json!({":p": {"BOOL": true}}),
            before_the_lookup: None,
            against_the_table: incorrect_operand("=", "BOOL"),
        },
        Row {
            case: "a number where begins_with needs a prefix",
            condition: "pk = :p AND begins_with(sk, :n)",
            values: json!({":p": {"S": "p"}, ":n": {"N": "1"}}),
            before_the_lookup: None,
            against_the_table: incorrect_operand("begins_with", "N"),
        },
        Row {
            case: "more clauses than the schema has key attributes",
            condition: "pk = :p AND sk = :s AND pk3 = :o",
            values: json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":o": {"S": "o"}}),
            before_the_lookup: None,
            against_the_table: "The number of query conditions (3) exceeds the number of key \
                                attributes defined in the schema (2)"
                .to_string(),
        },
        Row {
            // The recorded case: both binding defects in one condition, so neither can be
            // what makes the absent table answer.
            case: "a wrong-typed operand and a third clause together",
            condition: "pk = :p AND begins_with(sk, :n) AND pk3 = :o",
            values: json!({":p": {"S": "p"}, ":n": {"N": "1"}, ":o": {"S": "o"}}),
            before_the_lookup: None,
            against_the_table: incorrect_operand("begins_with", "N"),
        },
        Row {
            case: "an operand written before its operator, of a type no comparison orders",
            condition: ":p = pk",
            values: json!({":p": {"NULL": true}}),
            before_the_lookup: None,
            against_the_table: incorrect_operand("=", "NULL"),
        },
    ]
}

/// The `Query` a row sends against `table`. An unused substitution definition is its own
/// refusal, so a row defines only what its own condition writes — the alias row defines no
/// names at all, which is what makes `#undef` the undefined alias it is about.
fn request_for(row: &Row, table: &str) -> Value {
    json!({
        "TableName": table,
        "KeyConditionExpression": row.condition,
        "ExpressionAttributeValues": row.values,
    })
}

/// RULE: a request-settled defect is answered for an absent table; a binding defect is not,
/// because the table is resolved first.
#[tokio::test]
async fn an_absent_table_answers_before_the_key_condition_binds() {
    let router = emulator();
    for row in rows() {
        let request = request_for(&row, ABSENT);
        let (status, body) = call(&router, "Query", request).await;
        match &row.before_the_lookup {
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ValidationException"),
                    "case {:?}: a defect the request alone settles is refused before the \
                     table is resolved, got {status} with {body}",
                    row.case
                );
                assert_eq!(
                    &message(&body),
                    expected,
                    "case {:?}: expected the message {expected:?}, got {:?}",
                    row.case,
                    message(&body)
                );
            }
            None => {
                assert_eq!(
                    (status, error_code(&body).as_str(), message(&body).as_str()),
                    (400, "ResourceNotFoundException", NOT_FOUND),
                    "case {:?}: the absent table decides before the condition binds, got \
                     {status} with {body}",
                    row.case
                );
            }
        }
    }
}

/// RULE, the other half: against a table that IS there every row earns its own refusal, so
/// the binding rules are still stated — the lookup only orders them.
#[tokio::test]
async fn the_same_conditions_are_still_refused_once_the_table_is_there() {
    let router = seeded().await;
    for row in rows() {
        let request = request_for(&row, TABLE);
        let (status, body) = call(&router, "Query", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected the condition refused as a 400 ValidationException, got \
             {status} with {body}",
            row.case
        );
        assert_eq!(
            message(&body),
            row.against_the_table,
            "case {:?}: expected the message {:?}, got {:?}",
            row.case,
            row.against_the_table,
            message(&body)
        );
    }
}

/// RULE: the same ordering holds for a read addressed to a secondary INDEX of an absent
/// table — the table is what has to be resolved before any key schema is, so a binding
/// defect cannot be reported for a schema that does not exist.
#[tokio::test]
async fn an_absent_table_answers_before_an_index_key_schema_binds() {
    let router = emulator();
    for (case, condition, values) in [
        (
            "a wrong-typed operand on an index read",
            "gpk = :p",
            json!({":p": {"BOOL": false}}),
        ),
        (
            "a third clause on an index read",
            "gpk = :p AND gsk = :s AND pk3 = :o",
            json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":o": {"S": "o"}}),
        ),
    ] {
        let (status, body) = call(
            &router,
            "Query",
            json!({
                "TableName": ABSENT,
                "IndexName": "ByAnything",
                "KeyConditionExpression": condition,
                "ExpressionAttributeValues": values,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str(), message(&body).as_str()),
            (400, "ResourceNotFoundException", NOT_FOUND),
            "case {case:?}: the absent table decides first, got {status} with {body}"
        );
    }
}
