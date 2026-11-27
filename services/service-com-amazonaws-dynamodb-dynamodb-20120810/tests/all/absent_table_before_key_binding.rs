// SPDX-License-Identifier: CC0-1.0

//! WHEN the table a read is addressed to is resolved, relative to the defects its key
//! condition can carry.
//!
//! THE RULE: a `KeyConditionExpression` defect that the REQUEST alone settles — the
//! expression's syntax, a substitution token the expression uses but the request never
//! defined, and an operand outside the vocabulary the operator or function it was written with
//! has of its own (the three types DynamoDB orders for a comparison that orders its sides, a
//! String or Binary prefix for `begins_with`) — is refused before the addressed table is
//! resolved, so it is answered even for a table this account does not hold. Every other
//! key-condition rule is about how the condition BINDS to the key attributes of the schema it
//! is read against: how many comparisons that schema has key attributes for, which attributes
//! they name, and what an EQUALITY may compare a key declared of one type against — so each is
//! only reached once the table has been found, and a `Query` naming an absent table is refused
//! `ResourceNotFoundException` ("the operation tried to access a nonexistent table or index")
//! however many clauses it states.
//!
//! The rows span what the rule generalises over: which KIND of defect the condition carries
//! (syntax, an undefined value placeholder, an undefined name alias, an operand of a type no
//! comparison orders, an operand of a type `begins_with` has no prefix for, an equality
//! operand of a type no schema declares, more clauses than the schema has key attributes, and
//! the combinations of a request-settled defect with a binding one), whether the table is
//! absent or present, and whether the read is a `Query` on the table itself or on one of its
//! secondary indexes.
//!
//! The same rows against a table that IS there pin the other half: every row earns its own
//! refusal, so the lookup only orders them.

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

/// AWS's fixed sentence for an operand whose type is not the one the SCHEMA declares for
/// the key attribute it is compared with — which only the table can settle.
const SCHEMA_TYPE: &str = "One or more parameter values were invalid: Condition parameter \
                          type does not match schema type";

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
            // Equality compares ONE value, so what its operand's type is held to is the
            // type the SCHEMA declares for the key — a binding defect, which the table
            // has to be there to settle.
            case: "an equality operand of a type the schema does not declare",
            condition: "pk = :p",
            values: json!({":p": {"BOOL": true}}),
            before_the_lookup: None,
            against_the_table: SCHEMA_TYPE.to_string(),
        },
        Row {
            case: "a number where begins_with needs a prefix",
            condition: "pk = :p AND begins_with(sk, :n)",
            values: json!({":p": {"S": "p"}, ":n": {"N": "1"}}),
            before_the_lookup: Some(incorrect_operand("begins_with", "N")),
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
            // The recorded case: an equality operand of a type no key schema declares,
            // beside a clause only the schema can settle, with every placeholder defined —
            // so the absent table answers and the operand is reported only where there is a
            // schema to hold it to.
            case: "a wrong-typed operand and a third clause together",
            condition: "pk = :l AND sk = :s AND extra = :e",
            values: json!({
                ":l": {"L": [{"S": "x"}]},
                ":s": {"S": "a"},
                ":e": {"S": "e"},
            }),
            before_the_lookup: None,
            against_the_table: incorrect_operand("=", "L"),
        },
        Row {
            case: "an operand written before its operator, of a type no comparison orders",
            condition: ":p >= pk",
            values: json!({":p": {"NULL": true}}),
            before_the_lookup: Some(incorrect_operand(">=", "NULL")),
            against_the_table: incorrect_operand(">=", "NULL"),
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
/// table — a defect the request settles is answered whichever schema the read names, while a
/// defect of the CONDITION cannot be reported for a schema that does not exist.
#[tokio::test]
async fn an_absent_table_answers_before_an_index_key_schema_binds() {
    let router = emulator();
    // Each row: the condition, and what the rule requires of it against an absent table —
    // the request-settled defect, or the table's own refusal.
    for (case, condition, values, expected) in [
        (
            // A token nothing defines is the request's own defect, so it is answered
            // whichever schema the read names.
            "an undefined token on an index read",
            "gpk >= :undef",
            json!({}),
            (
                "ValidationException",
                "Invalid KeyConditionExpression: An expression attribute value used in \
                 expression is not defined; attribute value: :undef"
                    .to_string(),
            ),
        ),
        (
            // A comparison that ORDERS its sides orders the three types DynamoDB orders and
            // nothing else, which is the operator's own vocabulary and needs no schema.
            "an operand no comparison orders on an index read",
            "gpk >= :p",
            json!({":p": {"BOOL": false}}),
            (
                "ValidationException",
                "Invalid KeyConditionExpression: Incorrect operand type for operator or \
                 function; operator or function: >=, operand type: BOOL"
                    .to_string(),
            ),
        ),
        (
            "an equality operand of another type on an index read",
            "gpk = :p",
            json!({":p": {"BOOL": false}}),
            ("ResourceNotFoundException", NOT_FOUND.to_string()),
        ),
        (
            "a third clause on an index read",
            "gpk = :p AND gsk = :s AND pk3 = :o",
            json!({":p": {"S": "p"}, ":s": {"S": "a"}, ":o": {"S": "o"}}),
            ("ResourceNotFoundException", NOT_FOUND.to_string()),
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
            (status, error_code(&body), message(&body)),
            (400, expected.0.to_string(), expected.1.clone()),
            "case {case:?}: expected {expected:?}, got {status} with {body}"
        );
    }
}
