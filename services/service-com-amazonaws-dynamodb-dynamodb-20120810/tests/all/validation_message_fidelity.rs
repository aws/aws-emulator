// SPDX-License-Identifier: CC0-1.0

//! The TEXT a refusal carries, over the refusals whose wording AWS fixes, each
//! rule driven over a TABLE of inputs rather than one recorded request.
//!
//! Seven rules are under test, and every one of them is about the whole message:
//! a client branches on the error code and then matches the message, so a clause
//! AWS does not state — an interpolated type tag, an appended document path, a
//! missing envelope — is as much a divergence as a wrong status would be.
//!
//! 1. a `Query` that states neither of the two members a key condition can be
//!    written in is refused with AWS's fixed sentence for that pair, naming both
//!    members and nothing of the request;
//! 2. an `N` value that breaks one of the documented numeric rules — the text is
//!    not a number, the magnitude is outside the documented exponent range, the
//!    precision is past the documented 38 significant digits — is refused inside
//!    the `<N> validation error(s) detected: ` envelope, and the only part of the
//!    message that varies with the request is the offending text the unparsable
//!    clause echoes;
//! 3. an update action given an operand of a type its OPERATOR takes but the type the
//!    item holds cannot be combined with is refused with AWS's fixed text for that
//!    class, which names NEITHER the attribute nor either type tag — whichever action,
//!    whichever pair of types, and whether the update was written as an expression or
//!    as the legacy `AttributeUpdates` map (an operand outside the operator's own
//!    domain names the operator and its type set instead, which
//!    `tests/all/update_operand_type_sets.rs` states);
//! 4. an update operand that reads an attribute the item does not carry is refused
//!    with AWS's fixed text for that class, which does not name the path that was
//!    read;
//! 5. a substitution table that defines a key no expression referenced is refused
//!    naming the member that carried the keys and the keys themselves in one stable
//!    order, inside the envelope only where the request states its intent through an
//!    expression member whose refusals carry it;
//! 6. a key attribute whose value is not of the type the table's
//!    `AttributeDefinitions` declare for it is refused behind the parameter-values
//!    clause, over the key, the declared type and the supplied one;
//! 7. two document paths of one update that overlap are reported in the ORDER THE
//!    UPDATE STATES THEM: `path one` is the earlier action's path and `path two`
//!    the later one, whichever of the two is the deeper.
//!
//! An eighth rule is about the clause a DECLARED bound earns, which this service
//! states in one of two forms chosen by the operation that was served (see "Which
//! form a constraint clause takes" in `src/service.rs`): the item-request form
//! names the member as the model spells it on the wire and echoes no value, while
//! the request validator's form names it by its lowerCamel input path and echoes
//! the rejected value. Both are driven here side by side.
//!
//! Every bound and value set named below comes from the Smithy model or from the
//! documented quota it implements: `smithy_query(query="operation",
//! operation="Query")` gives `Limit -> PositiveIntegerObject @range{min:1}` and
//! `KeyConditions`/`KeyConditionExpression` both optional,
//! `smithy_query(query="shape", shape="com.amazonaws.dynamodb#TimeToLiveSpecification")`
//! gives `AttributeName @length{min:1,max:255}`, `ListBackups$Limit` is
//! `@range{min:1,max:100}` and `ListExports$MaxResults` `@range{min:1,max:25}`,
//! `KeyList` is `@length{min:1,max:100}`, and the 38 significant digits and the
//! 10^-130..10^125 exponent range are the "Service, Account, and Table Quotas"
//! figures for the `N` type.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The table every item row addresses: one string partition key.
const TABLE: &str = "Items";

/// The key of the seeded item every update row addresses, so no row is refused
/// for its key instead of for its expression.
fn key() -> Value {
    json!({"pk": {"S": "one"}})
}

/// The table and the one item the update rules are driven against: an attribute of
/// every type an update action can be given the wrong operand for.
async fn seeded() -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": TABLE, "Item": {
            "pk": {"S": "one"},
            "n": {"N": "5"},
            "s": {"S": "text"},
            "ss": {"SS": ["a", "b"]},
            "ns": {"NS": ["1", "2"]},
            "bs": {"BS": ["AQI="]},
            "l": {"L": [{"S": "x"}]},
            "m": {"M": {"kept": {"S": "y"}}},
        }}),
    )
    .await;
    router
}

/// Call `operation`, require a 400 `ValidationException`, and answer the message.
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
        "case {case:?} ({operation}): expected 400 ValidationException, got {status} {body}"
    );
    message(&body)
}

/// The whole message a single clause earns: the envelope over that clause.
fn envelope(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

// ===========================================================================
// 1. Query with neither member a key condition can be written in
// ===========================================================================

#[tokio::test]
async fn a_query_with_no_key_condition_states_the_fixed_sentence() {
    let router = seeded().await;

    // AWS's sentence for the pair, stated once here and compared whole: it names
    // both members and nothing of the request that was refused.
    let expected = "Either the KeyConditions or KeyConditionExpression parameter must be \
                    specified in the request.";

    // The rows differ in what ELSE the request carries, so the sentence cannot
    // depend on the rest of the request: nothing beside the table, a projection, a
    // filter, a page size, a legacy map that is present but empty.
    let rows: Vec<(&str, Value)> = vec![
        ("a query naming only its table", json!({"TableName": TABLE})),
        (
            "a query asking for consumed capacity but no key condition",
            json!({"TableName": TABLE, "ReturnConsumedCapacity": "TOTAL"}),
        ),
        (
            "a query with a filter but no key condition",
            json!({"TableName": TABLE, "FilterExpression": "attribute_exists(s)"}),
        ),
        (
            "a query with a page size but no key condition",
            json!({"TableName": TABLE, "Limit": 5}),
        ),
        (
            "a query whose legacy KeyConditions map is empty",
            json!({"TableName": TABLE, "KeyConditions": {}}),
        ),
        (
            "a consistent read with no key condition",
            json!({"TableName": TABLE, "ConsistentRead": true}),
        ),
        (
            "a reverse traversal with no key condition",
            json!({"TableName": TABLE, "ScanIndexForward": false}),
        ),
    ];

    for (case, request) in rows {
        let stated = refusal(&router, case, "Query", request).await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }

    // The control: a query that DOES state one of the two members is served, so the
    // refusals above are about the missing key condition and nothing else.
    for request in [
        json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p",
               "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        json!({"TableName": TABLE,
               "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                        "AttributeValueList": [{"S": "one"}]}}}),
    ] {
        let served = ok(&router, "Query", request).await;
        assert_eq!(
            served["Count"],
            json!(1),
            "case a query stating one key condition: expected the seeded item, got {served}"
        );
    }
}

// ===========================================================================
// 2. The numeric rules, inside the validation envelope
// ===========================================================================

/// 39 significant digits: one past the documented precision.
const THIRTY_NINE_DIGITS: &str = "123456789012345678901234567890123456789";

#[tokio::test]
async fn a_number_outside_the_documented_rules_is_stated_inside_the_envelope() {
    let router = seeded().await;

    // Each row: the case, the `N` text, and the whole clause the envelope carries.
    // The rows span the three documented rules (the text is no number, the
    // magnitude is outside the exponent range either way, the precision is past 38
    // significant digits), the forms an unparsable text takes, and a mantissa that
    // normalises to zero under an exponent outside the documented range.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an exponent with no mantissa",
            "e2".to_string(),
            "The parameter cannot be converted to a numeric value: e2".to_string(),
        ),
        (
            "a word",
            "abc".to_string(),
            "The parameter cannot be converted to a numeric value: abc".to_string(),
        ),
        (
            "an empty text",
            String::new(),
            "The parameter cannot be converted to a numeric value: ".to_string(),
        ),
        (
            "a sign with no digits",
            "-".to_string(),
            "The parameter cannot be converted to a numeric value: -".to_string(),
        ),
        (
            "two decimal points",
            "1.2.3".to_string(),
            "The parameter cannot be converted to a numeric value: 1.2.3".to_string(),
        ),
        (
            "a hexadecimal spelling",
            "0x10".to_string(),
            "The parameter cannot be converted to a numeric value: 0x10".to_string(),
        ),
        (
            "a magnitude past the documented exponent maximum",
            "1e130".to_string(),
            "Number overflow. Attempting to store a number with magnitude larger than \
             supported range"
                .to_string(),
        ),
        (
            "a magnitude below the documented exponent minimum",
            "1e-140".to_string(),
            "Number underflow. Attempting to store a number with magnitude smaller than \
             supported range"
                .to_string(),
        ),
        (
            "one digit more than the documented precision",
            THIRTY_NINE_DIGITS.to_string(),
            "Attempting to store more than 38 significant digits in a Number".to_string(),
        ),
        (
            // The mantissa normalises to zero, and the exponent the text spelled is
            // still outside the documented range.
            "a zero written with an exponent past the documented maximum",
            "0e126".to_string(),
            "Number overflow. Attempting to store a number with magnitude larger than \
             supported range"
                .to_string(),
        ),
        (
            "a zero written with an exponent below the documented minimum",
            "0e-131".to_string(),
            "Number underflow. Attempting to store a number with magnitude smaller than \
             supported range"
                .to_string(),
        ),
    ];

    for (case, text, clause) in rows {
        let expected = envelope(&clause);
        // The same value is refused the same way wherever it appears: as the whole
        // attribute, as a member of a number set, and nested inside a list.
        for (position, item) in [
            ("a top-level N", json!({"pk": {"S": "two"}, "v": {"N": text}})),
            (
                "an NS member",
                json!({"pk": {"S": "two"}, "v": {"NS": [text]}}),
            ),
            (
                "an N nested in a list",
                json!({"pk": {"S": "two"}, "v": {"L": [{"N": text}]}}),
            ),
        ] {
            let case = format!("{case} as {position}");
            let stated =
                refusal(&router, &case, "PutItem", json!({"TableName": TABLE, "Item": item})).await;
            assert_eq!(
                stated, expected,
                "case {case:?}: expected the message {expected:?}, got {stated:?}"
            );
        }
    }

    // The refused writes left nothing behind: every row addressed the same key.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {"pk": {"S": "two"}}}),
    )
    .await;
    assert_eq!(
        read.get("Item"),
        None,
        "case a refused number: no item is written, got {read}"
    );
}

// ===========================================================================
// 3. An update operand whose type the STORED attribute cannot be combined with
// ===========================================================================

/// AWS's fixed text for that class, which names neither operand.
const OPERAND_TYPE: &str = "An operand in the update expression has an incorrect data type";

#[tokio::test]
async fn an_update_operand_of_the_wrong_type_states_the_fixed_text() {
    let router = seeded().await;

    // Each row: the case and the UpdateItem request. The rows span the action
    // (`ADD`, `DELETE`), the stored type the action was applied to (a set of each
    // kind, a number, a string, a list and a map), the operand's own type, and the
    // SURFACE the update was written on (the expression and the legacy
    // `AttributeUpdates` map).
    //
    // Every operand here is one its OPERATOR takes — a number or a set for `ADD`, a set
    // for `DELETE` — and what refuses it is the type the item holds at that path. An
    // operand outside the operator's own domain is refused naming the operator and its
    // type set instead, which `tests/all/update_operand_type_sets.rs` states.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an ADD of a number set onto a string set",
            json!({"UpdateExpression": "ADD ss :v",
                   "ExpressionAttributeValues": {":v": {"NS": ["3"]}}}),
        ),
        (
            "an ADD of a string set onto a number set",
            json!({"UpdateExpression": "ADD ns :v",
                   "ExpressionAttributeValues": {":v": {"SS": ["c"]}}}),
        ),
        (
            "an ADD of a binary set onto a string set",
            json!({"UpdateExpression": "ADD ss :v",
                   "ExpressionAttributeValues": {":v": {"BS": ["AQI="]}}}),
        ),
        (
            "an ADD of a string set onto a binary set",
            json!({"UpdateExpression": "ADD bs :v",
                   "ExpressionAttributeValues": {":v": {"SS": ["c"]}}}),
        ),
        (
            "an ADD of a set onto a number",
            json!({"UpdateExpression": "ADD n :v",
                   "ExpressionAttributeValues": {":v": {"SS": ["c"]}}}),
        ),
        (
            "an ADD of a number onto a string",
            json!({"UpdateExpression": "ADD s :v",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
        ),
        (
            "an ADD of a number onto a list",
            json!({"UpdateExpression": "ADD l :v",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
        ),
        (
            "an ADD of a number onto a map",
            json!({"UpdateExpression": "ADD m :v",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
        ),
        (
            "a DELETE of a number set from a string set",
            json!({"UpdateExpression": "DELETE ss :v",
                   "ExpressionAttributeValues": {":v": {"NS": ["1"]}}}),
        ),
        (
            "a DELETE of a string set from a binary set",
            json!({"UpdateExpression": "DELETE bs :v",
                   "ExpressionAttributeValues": {":v": {"SS": ["a"]}}}),
        ),
        (
            "a DELETE of a set from a number",
            json!({"UpdateExpression": "DELETE n :v",
                   "ExpressionAttributeValues": {":v": {"SS": ["a"]}}}),
        ),
        (
            "the legacy map's ADD of a number onto a string",
            json!({"AttributeUpdates": {"s": {"Action": "ADD", "Value": {"N": "1"}}}}),
        ),
        (
            "the legacy map's ADD of a number set onto a string set",
            json!({"AttributeUpdates": {"ss": {"Action": "ADD", "Value": {"NS": ["3"]}}}}),
        ),
        (
            "the legacy map's DELETE of a number set from a string set",
            json!({"AttributeUpdates": {"ss": {"Action": "DELETE", "Value": {"NS": ["3"]}}}}),
        ),
    ];

    for (case, update) in rows {
        let mut request = json!({"TableName": TABLE, "Key": key()});
        for (member, value) in update.as_object().expect("an object") {
            request[member] = value.clone();
        }
        let stated = refusal(&router, case, "UpdateItem", request).await;
        assert_eq!(
            stated, OPERAND_TYPE,
            "case {case:?}: expected the operand-type text, got {stated:?}"
        );
        // The sentence is fixed: no `AttributeValue` type tag the request carried,
        // and no attribute name of it, is interpolated into the message.
        for leaked in ["SS", "NS", "BS", "BOOL", "Type mismatch"] {
            assert!(
                !stated.contains(leaked),
                "case {case:?}: expected nothing of the request in the message, got \
                 {leaked:?} in {stated:?}"
            );
        }
    }

    // Every refusal above left the seeded item exactly as it was, read back through
    // the implemented reader.
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": key(), "ConsistentRead": true}),
    )
    .await;
    assert_eq!(
        read["Item"]["ss"],
        json!({"SS": ["a", "b"]}),
        "case a refused ADD: the stored set is untouched, got {read}"
    );
    assert_eq!(
        read["Item"]["n"],
        json!({"N": "5"}),
        "case a refused ADD: the stored number is untouched, got {read}"
    );
    assert_eq!(
        read["Item"].get("fresh"),
        None,
        "case a refused ADD: no attribute is created, got {read}"
    );
}

// ===========================================================================
// 4. An update operand that reads an attribute the item does not carry
// ===========================================================================

/// AWS's fixed text for that class, which names no path.
const MISSING_OPERAND: &str =
    "The provided expression refers to an attribute that does not exist in the item";

#[tokio::test]
async fn an_update_operand_reading_a_missing_attribute_names_no_path() {
    let router = seeded().await;

    // The rows span where the missing operand sits (the left and the right side of
    // either arithmetic operator, and a plain assignment), and the SHAPE of the path
    // that addresses nothing (a top-level name, a map entry, a list index past the
    // end).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a sum whose left operand is missing",
            "gone",
            json!({"UpdateExpression": "SET tally = gone + :one",
                   "ExpressionAttributeValues": {":one": {"N": "1"}}}),
        ),
        (
            "a sum whose right operand is missing",
            "gone",
            json!({"UpdateExpression": "SET tally = n + gone"}),
        ),
        (
            "a difference whose right operand is missing",
            "absent",
            json!({"UpdateExpression": "SET tally = :one - absent",
                   "ExpressionAttributeValues": {":one": {"N": "1"}}}),
        ),
        (
            "a difference whose left operand is missing",
            "absent",
            json!({"UpdateExpression": "SET tally = absent - :one",
                   "ExpressionAttributeValues": {":one": {"N": "1"}}}),
        ),
        (
            "an assignment from an attribute that is not there",
            "nowhere",
            json!({"UpdateExpression": "SET mirrored = nowhere"}),
        ),
        (
            "an assignment from a map entry that is not there",
            "gone",
            json!({"UpdateExpression": "SET mirrored = m.gone"}),
        ),
        (
            "an assignment from a list index past the end",
            "l[4]",
            json!({"UpdateExpression": "SET mirrored = l[4]"}),
        ),
    ];

    for (case, path, update) in rows {
        let mut request = json!({"TableName": TABLE, "Key": key()});
        for (member, value) in update.as_object().expect("an object") {
            request[member] = value.clone();
        }
        let stated = refusal(&router, case, "UpdateItem", request).await;
        assert_eq!(
            stated, MISSING_OPERAND,
            "case {case:?}: expected the missing-operand text, got {stated:?}"
        );
        assert!(
            !stated.contains(path),
            "case {case:?}: expected the path {path:?} not to be named, got {stated:?}"
        );
    }
}

// ===========================================================================
// 5. A substitution key no expression referenced
// ===========================================================================

#[tokio::test]
async fn an_unreferenced_substitution_key_is_stated_as_the_service_words_it() {
    let router = seeded().await;

    // Each row: the case, the operation, the request, the expression member the
    // request states its intent through, the member that carried the surplus keys
    // and those keys in the order the message states them. The rows span the member
    // (names and values), the operation (an item write, an item read, a query, a
    // scan), the expression the keys were offered to (an update, a condition, a
    // projection, a filter, a key condition) and the number of surplus keys.
    let rows: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "a surplus value beside an update",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(), "UpdateExpression": "SET s = :v",
                   "ExpressionAttributeValues": {":v": {"S": "new"}, ":extra": {"S": "x"}}}),
            "UpdateExpression",
            "ExpressionAttributeValues",
            ":extra",
        ),
        (
            "a surplus name beside an update",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(), "UpdateExpression": "SET #s = :v",
                   "ExpressionAttributeNames": {"#s": "s", "#other": "n"},
                   "ExpressionAttributeValues": {":v": {"S": "new"}}}),
            "UpdateExpression",
            "ExpressionAttributeNames",
            "#other",
        ),
        (
            "two surplus names, reported in one stable order",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(), "UpdateExpression": "SET #s = :v",
                   "ExpressionAttributeNames": {"#s": "s", "#b": "n", "#a": "l"},
                   "ExpressionAttributeValues": {":v": {"S": "new"}}}),
            "UpdateExpression",
            "ExpressionAttributeNames",
            "#a, #b",
        ),
        (
            "a surplus value beside a condition",
            "DeleteItem",
            json!({"TableName": TABLE, "Key": key(), "ConditionExpression": "s = :v",
                   "ExpressionAttributeValues": {":v": {"S": "text"}, ":extra": {"S": "x"}}}),
            "ConditionExpression",
            "ExpressionAttributeValues",
            ":extra",
        ),
        (
            "a surplus name beside a read's projection",
            "GetItem",
            json!({"TableName": TABLE, "Key": key(), "ProjectionExpression": "#s",
                   "ExpressionAttributeNames": {"#s": "s", "#other": "n"}}),
            "ProjectionExpression",
            "ExpressionAttributeNames",
            "#other",
        ),
        (
            "a surplus value beside a query's key condition",
            "Query",
            json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}, ":extra": {"N": "1"}}}),
            "KeyConditionExpression",
            "ExpressionAttributeValues",
            ":extra",
        ),
        (
            "a surplus name beside a scan's filter",
            "Scan",
            json!({"TableName": TABLE, "FilterExpression": "attribute_exists(#s)",
                   "ExpressionAttributeNames": {"#s": "s", "#other": "n"}}),
            "FilterExpression",
            "ExpressionAttributeNames",
            "#other",
        ),
    ];

    for (case, operation, request, expression_member, member, keys) in rows {
        let expected = crate::undefined_expression_tokens::unused_entries(
            expression_member,
            member,
            &[keys],
        );
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }
}

// ===========================================================================
// 6. A key attribute of a type the table does not declare for it
// ===========================================================================

#[tokio::test]
async fn a_key_of_another_type_is_stated_behind_the_parameter_values_clause() {
    let router = emulator();
    create_active_table(&router, "Letters", &[("pk", "HASH", "S")]).await;
    create_active_table(&router, "Counters", &[("pk", "HASH", "N")]).await;
    create_active_table(&router, "Blobs", &[("pk", "HASH", "B")]).await;
    create_active_table(
        &router,
        "Pairs",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
    )
    .await;

    // Each row: the case, the table, the item written, the key named and the
    // declared and supplied types the clause must state. The rows span the declared
    // type (`S`, `N`, `B`), the supplied type (another scalar, a set, a boolean, a
    // list, a map) and the key's ROLE in the schema (the partition key and the sort
    // key of a composite one).
    let rows: Vec<(&str, &str, Value, &str, &str, &str)> = vec![
        (
            "a number where the schema declares a string",
            "Letters",
            json!({"pk": {"N": "7"}}),
            "pk",
            "S",
            "N",
        ),
        (
            "binary where the schema declares a string",
            "Letters",
            json!({"pk": {"B": "AQI="}}),
            "pk",
            "S",
            "B",
        ),
        (
            "a string set where the schema declares a string",
            "Letters",
            json!({"pk": {"SS": ["a"]}}),
            "pk",
            "S",
            "SS",
        ),
        (
            "a boolean where the schema declares a string",
            "Letters",
            json!({"pk": {"BOOL": true}}),
            "pk",
            "S",
            "BOOL",
        ),
        (
            "a list where the schema declares a string",
            "Letters",
            json!({"pk": {"L": [{"S": "a"}]}}),
            "pk",
            "S",
            "L",
        ),
        (
            "a string where the schema declares a number",
            "Counters",
            json!({"pk": {"S": "seven"}}),
            "pk",
            "N",
            "S",
        ),
        (
            "a map where the schema declares a number",
            "Counters",
            json!({"pk": {"M": {"inner": {"N": "1"}}}}),
            "pk",
            "N",
            "M",
        ),
        (
            "a string where the schema declares binary",
            "Blobs",
            json!({"pk": {"S": "AQI="}}),
            "pk",
            "B",
            "S",
        ),
        (
            "a sort key of another type than the schema declares",
            "Pairs",
            json!({"pk": {"S": "ok"}, "sk": {"S": "7"}}),
            "sk",
            "N",
            "S",
        ),
    ];

    for (case, table, item, attribute, declared, supplied) in rows {
        let expected = format!(
            "One or more parameter values were invalid: Type mismatch for key {attribute} \
             expected: {declared} actual: {supplied}"
        );
        let stated = refusal(
            &router,
            case,
            "PutItem",
            json!({"TableName": table, "Item": item}),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }
}

// ===========================================================================
// 7. Two overlapping document paths, in the order the update states them
// ===========================================================================

#[tokio::test]
async fn two_overlapping_paths_are_stated_in_the_order_the_update_states_them() {
    let router = seeded().await;

    // Each row: the case, the update expression, and the two paths the message must
    // state in that order. The rows span which action carries which path, the
    // DIRECTION of the nesting (the deeper path stated first and second), the depth
    // (two and three steps), the step's kind (a map entry and a list index) and the
    // case where both actions name ONE location.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a write inside a removal, the deeper path first",
            "SET m.kept = :v REMOVE m",
            "[m, kept]",
            "[m]",
        ),
        (
            "the same pair with the removal stated first",
            "REMOVE m SET m.kept = :v",
            "[m]",
            "[m, kept]",
        ),
        (
            "a write inside another write, the shallower path first",
            "SET m.kept = :v, m.kept.deeper = :v",
            "[m, kept]",
            "[m, kept, deeper]",
        ),
        (
            "the same pair with the deeper path first",
            "SET m.kept.deeper = :v, m.kept = :v",
            "[m, kept, deeper]",
            "[m, kept]",
        ),
        (
            "a list element inside its list",
            "SET l[0] = :v REMOVE l",
            "[l, [0]]",
            "[l]",
        ),
        (
            "a list inside its element's removal",
            "REMOVE l[0] SET l = :v",
            "[l, [0]]",
            "[l]",
        ),
        (
            "two actions naming one location",
            "SET s = :v REMOVE s",
            "[s]",
            "[s]",
        ),
    ];

    for (case, expression, one, two) in rows {
        let expected = envelope(&format!(
            "Invalid UpdateExpression: Two document paths overlap with each other; must \
             remove or rewrite one of these paths; path one: {one}, path two: {two}"
        ));
        let stated = refusal(
            &router,
            case,
            "UpdateItem",
            json!({"TableName": TABLE, "Key": key(), "UpdateExpression": expression,
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
        )
        .await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }
}

// ===========================================================================
// 8. The two forms a DECLARED bound's clause takes
// ===========================================================================

/// `count` distinct keys of the single-key table, for the batched read's cap.
fn keys(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"pk": {"S": format!("k{index}")}}))
        .collect()
}

#[tokio::test]
async fn a_declared_bound_is_stated_in_the_served_operations_form() {
    let router = seeded().await;

    // Each row: the case, the operation, the request and the whole message. The
    // rows put the two forms side by side over the same violation class and, for
    // `Limit`, the same member NAME: the item-request form names the member as the
    // model spells it with nothing echoed, the request validator's form names the
    // lowerCamel input path and echoes the rejected value.
    let long_name = "a".repeat(256);
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a query page size one below its range minimum",
            "Query",
            json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p", "Limit": 0,
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
            envelope(
                "Value at 'Limit' failed to satisfy constraint: Member must have value \
                 greater than or equal to 1",
            ),
        ),
        (
            "the same member far below the same bound states the same clause",
            "Query",
            json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p", "Limit": -7,
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
            envelope(
                "Value at 'Limit' failed to satisfy constraint: Member must have value \
                 greater than or equal to 1",
            ),
        ),
        (
            "a batched key list one past its 100-key cap",
            "BatchGetItem",
            json!({"RequestItems": {TABLE: {"Keys": keys(101)}}}),
            envelope(
                "Value at 'RequestItems.Items.member.Keys' failed to satisfy constraint: \
                 Member must have length less than or equal to 100",
            ),
        ),
        (
            "a backup listing page size one below its range minimum",
            "ListBackups",
            json!({"Limit": 0}),
            envelope(
                "Value '0' at 'limit' failed to satisfy constraint: Member must have value \
                 greater than or equal to 1",
            ),
        ),
        (
            "an export listing page size one past its range maximum",
            "ListExports",
            json!({"MaxResults": 26}),
            envelope(
                "Value '26' at 'maxResults' failed to satisfy constraint: Member must have \
                 value less than or equal to 25",
            ),
        ),
        (
            "an empty TTL attribute name, one below its length minimum",
            "UpdateTimeToLive",
            json!({"TableName": TABLE,
                   "TimeToLiveSpecification": {"Enabled": true, "AttributeName": ""}}),
            envelope(
                "Value '' at 'timeToLiveSpecification.attributeName' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1",
            ),
        ),
        (
            "a TTL attribute name one past its length maximum",
            "UpdateTimeToLive",
            json!({"TableName": TABLE,
                   "TimeToLiveSpecification": {"Enabled": true, "AttributeName": long_name}}),
            envelope(&format!(
                "Value '{long_name}' at 'timeToLiveSpecification.attributeName' failed to \
                 satisfy constraint: Member must have length less than or equal to 255"
            )),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let stated = refusal(&router, case, operation, request).await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // The control for both forms: a value AT each of those bounds is no constraint
    // violation at all, so the clauses above are the bound and not an off-by-one.
    for (case, operation, request) in [
        (
            "a query page size at its minimum",
            "Query",
            json!({"TableName": TABLE, "KeyConditionExpression": "pk = :p", "Limit": 1,
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
        ),
        (
            "a batched key list at its cap",
            "BatchGetItem",
            json!({"RequestItems": {TABLE: {"Keys": keys(100)}}}),
        ),
        ("a backup listing page size at its minimum", "ListBackups", json!({"Limit": 1})),
        (
            "an export listing page size at its maximum",
            "ListExports",
            json!({"MaxResults": 25}),
        ),
        (
            "a TTL attribute name at its length minimum",
            "UpdateTimeToLive",
            json!({"TableName": TABLE,
                   "TimeToLiveSpecification": {"Enabled": true, "AttributeName": "e"}}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({operation}): a value at a published bound is served, got \
             {status} {body}"
        );
    }
}
