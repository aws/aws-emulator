// SPDX-License-Identifier: CC0-1.0

//! What this service answers a request it refuses, and what one update expression reads.
//!
//! Six rules are under test, each over a TABLE of inputs rather than one request:
//!
//! 1. an `AttributeValue` carries EXACTLY one of the ten datatypes
//!    `smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`
//!    declares (`S N B SS NS BS M L NULL BOOL`), and a value with none or with more
//!    than one is refused by the datatype rule, naming the API concept and not the wire
//!    encoding;
//! 2. a refusal the EXPRESSION language raises names the expression member it came from
//!    and builds its detail from the request — the offending path as a bracketed token
//!    list, the undefined alias, the operator or function and the type of the operand it
//!    would not take, the token the parse stopped at, the reserved word;
//! 3. a document path with no parent is refused with AWS's FIXED sentence, with nothing
//!    of the path appended;
//! 4. every clause of one update resolves its document paths against the item as it
//!    stood BEFORE the update, and `UPDATED_NEW` reports the value at each path the
//!    expression NAMED;
//! 5. `size()` of a String is its length in UTF-8 BYTES, which is what the condition a
//!    write is gated by compares;
//! 6. a modeled error names itself on the wire by its API-version-qualified shape id,
//!    and the error CLASS follows what the caller can do about the request: a parameter
//!    the caller must change is a `ValidationException`, and a limit on the work one
//!    call may ask for is `LimitExceededException`.
//!
//! The rows differ along what each rule generalises over: the datatype, the expression
//! member, the shape of the document path, the position and count of the clauses, the
//! byte width of the characters in a string, and the operation that refuses.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok, REGION};
use crate::table_items::create_active_table;

/// Call `operation`, require a 400 with `expected_code`, and report the message.
async fn refusal(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
    expected_code: &str,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, expected_code),
        "case {case:?} ({operation}): expected 400 {expected_code}, got {status} {body}"
    );
    message(&body)
}

/// AWS's fixed text for an `AttributeValue` that carries no datatype at all.
const EMPTY_VALUE: &str =
    "Supplied AttributeValue is empty, must contain exactly one of the supported datatypes";

/// AWS's fixed text for one that carries more than one.
const AMBIGUOUS_VALUE: &str = "Supplied AttributeValue has more than one datatypes set, must \
                               contain exactly one of the supported datatypes";

#[tokio::test]
async fn an_attribute_value_carries_exactly_one_datatype() {
    let router = emulator();
    create_active_table(&router, "Values", &[("pk", "HASH", "S")]).await;

    // One row per datatype the model declares, each a WELL-FORMED value of that type:
    // the rule is about arity, so every one of them is accepted.
    let datatypes: Vec<(&str, Value)> = vec![
        ("S", json!({"S": "text"})),
        ("N", json!({"N": "1.5"})),
        ("B", json!({"B": "dGhpcw=="})),
        ("SS", json!({"SS": ["a", "b"]})),
        ("NS", json!({"NS": ["1", "2"]})),
        ("BS", json!({"BS": ["dGhpcw=="]})),
        ("M", json!({"M": {"inner": {"S": "text"}}})),
        ("L", json!({"L": [{"S": "text"}, {"N": "2"}]})),
        ("NULL", json!({"NULL": true})),
        ("BOOL", json!({"BOOL": false})),
    ];
    for (datatype, value) in &datatypes {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Values",
                   "Item": {"pk": {"S": format!("one-{datatype}")}, "v": value}}),
        )
        .await;
        // Read it back: the datatype's wire NAME and its read and write arms are what
        // the round trip exercises, so a member that drifted from the model — renamed,
        // reordered or dropped — fails here rather than silently reading as another.
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "Values", "Key": {"pk": {"S": format!("one-{datatype}")}}}),
        )
        .await;
        assert_eq!(
            read["Item"]["v"], *value,
            "case the {datatype} datatype: expected it to read back as it was stored, got {}",
            read["Item"]
        );
    }

    // A value with NO datatype, at each position an `AttributeValue` can stand in: a
    // top-level attribute, inside a map, inside a list, and as the key itself.
    let empty: Vec<(&str, Value)> = vec![
        (
            "an empty value as a top-level attribute",
            json!({"TableName": "Values", "Item": {"pk": {"S": "k"}, "v": {}}}),
        ),
        (
            "an empty value inside a map",
            json!({"TableName": "Values",
                   "Item": {"pk": {"S": "k"}, "v": {"M": {"inner": {}}}}}),
        ),
        (
            "an empty value inside a list",
            json!({"TableName": "Values",
                   "Item": {"pk": {"S": "k"}, "v": {"L": [{"S": "ok"}, {}]}}}),
        ),
        (
            "an empty value as the key",
            json!({"TableName": "Values", "Item": {"pk": {}}}),
        ),
        (
            "an empty value in an expression's substitution map",
            json!({"TableName": "Values", "Key": {"pk": {"S": "k"}},
                   "UpdateExpression": "SET v = :v",
                   "ExpressionAttributeValues": {":v": {}}}),
        ),
    ];
    for (case, request) in empty {
        let operation = if request.get("Key").is_some() {
            "UpdateItem"
        } else {
            "PutItem"
        };
        let stated = refusal(&router, case, operation, request, "ValidationException").await;
        assert_eq!(
            stated, EMPTY_VALUE,
            "case {case:?}: expected the datatype rule's text, got {stated:?}"
        );
    }

    // A value with TWO datatypes, over several pairs: the same rule from the other side.
    for (first, second) in [
        (json!({"S": "text"}), json!({"N": "1"})),
        (json!({"BOOL": true}), json!({"NULL": true})),
        (json!({"L": []}), json!({"M": {}})),
    ] {
        let mut value = first.as_object().expect("an object").clone();
        for (datatype, held) in second.as_object().expect("an object") {
            value.insert(datatype.clone(), held.clone());
        }
        let case = format!("a value carrying {first} and {second}");
        let stated = refusal(
            &router,
            &case,
            "PutItem",
            json!({"TableName": "Values", "Item": {"pk": {"S": "k"}, "v": value}}),
            "ValidationException",
        )
        .await;
        assert_eq!(
            stated, AMBIGUOUS_VALUE,
            "case {case:?}: expected the datatype rule's text, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn an_expression_refusal_names_its_member_and_the_request_detail() {
    let router = emulator();
    create_active_table(&router, "Expressions", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Expressions", "Item": {
            "pk": {"S": "one"},
            "l": {"L": [{"S": "a"}, {"S": "b"}]},
            "m": {"M": {"kept": {"S": "x"}}},
            "s": {"S": "text"},
        }}),
    )
    .await;

    // Each row: the case, the operation, the request, and the WHOLE message. The rows
    // span the detail forms the rule builds from the request — the overlapping paths as
    // AWS's bracketed token lists (at one and at two steps, over a list index and over
    // a map entry), the undefined alias, the operator or function with the type of the
    // operand it would not take, the token the parse stopped at, and the reserved word.
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "two clauses writing one list index",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "REMOVE l[1] SET l[1] = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Two document paths overlap \
             with each other; must remove or rewrite one of these paths; path one: [l, [1]], \
             path two: [l, [1]]"
                .to_string(),
        ),
        (
            "two clauses writing one map entry",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "REMOVE m.kept SET m.kept = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Two document paths overlap \
             with each other; must remove or rewrite one of these paths; path one: [m, kept], \
             path two: [m, kept]"
                .to_string(),
        ),
        (
            "one clause's path inside another's",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET m = :v, m.kept = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Two document paths overlap \
             with each other; must remove or rewrite one of these paths; path one: [m], path \
             two: [m, kept]"
                .to_string(),
        ),
        (
            "the same two paths stated the other way round",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET m.kept = :v, m = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Two document paths overlap \
             with each other; must remove or rewrite one of these paths; path one: [m, kept], \
             path two: [m]"
                .to_string(),
        ),
        (
            "an undefined #name in a condition",
            "DeleteItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "ConditionExpression": "attribute_exists(#absent)"}),
            "1 validation error detected: Invalid ConditionExpression: An expression attribute \
             name used in the document path is not defined; attribute name: #absent"
                .to_string(),
        ),
        (
            "an undefined #name in an update",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET #absent = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: An expression attribute \
             name used in the document path is not defined; attribute name: #absent"
                .to_string(),
        ),
        (
            "list_append given a string second operand",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET l = list_append(l, :v)",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Incorrect operand type for \
             operator or function; operator or function: list_append, operand type: S"
                .to_string(),
        ),
        (
            "list_append given a number FIRST operand",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET l = list_append(:v, l)",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
            "1 validation error detected: Invalid UpdateExpression: Incorrect operand type for \
             operator or function; operator or function: list_append, operand type: N"
                .to_string(),
        ),
        (
            "arithmetic given a string operand",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET n = s + :v",
                   "ExpressionAttributeValues": {":v": {"N": "1"}}}),
            "1 validation error detected: Invalid UpdateExpression: Incorrect operand type for \
             operator or function; operator or function: +, operand type: S"
                .to_string(),
        ),
        (
            "an update that stops at the end of the expression",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET x ="}),
            "1 validation error detected: Invalid UpdateExpression: Syntax error; token: \
             \"<EOF>\", near: \"=\""
                .to_string(),
        ),
        (
            "an update that stops at a token in the middle",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET x = , :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Syntax error; token: \",\", \
             near: \"= , :v\""
                .to_string(),
        ),
        (
            "a bare reserved word as an attribute name in an update",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET comment = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Attribute name is a \
             reserved keyword; reserved keyword: comment"
                .to_string(),
        ),
        (
            // `ZONE` is the last entry of the published reserved-word list, which the
            // issue's own example (`name`) is nowhere near.
            "a bare reserved word from the end of the list",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET zone = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Attribute name is a \
             reserved keyword; reserved keyword: zone"
                .to_string(),
        ),
        (
            "a bare reserved word inside a document path",
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET m.timestamp = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "1 validation error detected: Invalid UpdateExpression: Attribute name is a \
             reserved keyword; reserved keyword: timestamp"
                .to_string(),
        ),
        (
            "a bare reserved word in a condition",
            "DeleteItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "ConditionExpression": "attribute_exists(status)"}),
            "1 validation error detected: Invalid ConditionExpression: Attribute name is a \
             reserved keyword; reserved keyword: status"
                .to_string(),
        ),
    ];
    for (case, operation, request, expected) in cases {
        let stated = refusal(&router, case, operation, request, "ValidationException").await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // The members a READ narrows its result with are validated too, and AWS states
    // their refusals WITHOUT the envelope: the clause stands on its own.
    let read_members: Vec<(&str, &str, Value, String)> = vec![
        (
            "a bare reserved word in a filter",
            "Scan",
            json!({"TableName": "Expressions", "FilterExpression": "comment = :v",
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
            "Invalid FilterExpression: Attribute name is a reserved keyword; reserved keyword: \
             comment"
                .to_string(),
        ),
        (
            "an undefined #name in a filter",
            "Query",
            json!({"TableName": "Expressions", "KeyConditionExpression": "pk = :p",
                   "FilterExpression": "#absent = :v",
                   "ExpressionAttributeValues": {":p": {"S": "one"}, ":v": {"S": "X"}}}),
            "Invalid FilterExpression: An expression attribute name used in the document path \
             is not defined; attribute name: #absent"
                .to_string(),
        ),
        (
            "a bare reserved word in a projection",
            "GetItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "ProjectionExpression": "zone, pk"}),
            "Invalid ProjectionExpression: Attribute name is a reserved keyword; reserved \
             keyword: zone"
                .to_string(),
        ),
        (
            "a projection that is not a list of paths",
            "Scan",
            json!({"TableName": "Expressions", "ProjectionExpression": "a b"}),
            "Invalid ProjectionExpression: Syntax error; token: \"b\", near: \"a b\"".to_string(),
        ),
        (
            // The key condition is the one expression grammar the `Query` handler parses
            // itself; the reserved-word rule is the same rule, in that member's words.
            "a bare reserved word as a key attribute in a key condition",
            "Query",
            json!({"TableName": "Expressions",
                   "KeyConditionExpression": "pk = :p AND timestamp > :t",
                   "ExpressionAttributeValues": {":p": {"S": "one"}, ":t": {"N": "1"}}}),
            "Invalid KeyConditionExpression: Attribute name is a reserved keyword; reserved \
             keyword: timestamp"
                .to_string(),
        ),
        (
            // A key condition the grammar cannot read is a SYNTAX defect: the reserved
            // word is not yet an attribute name in an expression that never parsed.
            "an unparsable key condition carrying a reserved word",
            "Query",
            json!({"TableName": "Expressions",
                   "KeyConditionExpression": "pk = :p AND timestamp",
                   "ExpressionAttributeValues": {":p": {"S": "one"}}}),
            "Invalid KeyConditionExpression: Syntax error; token: \"<EOF>\", near: \"timestamp\""
                .to_string(),
        ),
    ];
    for (case, operation, request, expected) in read_members {
        let stated = refusal(&router, case, operation, request, "ValidationException").await;
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected the message {expected:?}, got {stated:?}"
        );
    }

    // A name the expression supplies through `ExpressionAttributeNames` is NOT the
    // keyword, however reserved the word is: that is what the member is for.
    for word in ["comment", "zone", "status", "timestamp"] {
        ok(
            &router,
            "UpdateItem",
            json!({"TableName": "Expressions", "Key": {"pk": {"S": "one"}},
                   "UpdateExpression": "SET #aliased = :v",
                   "ExpressionAttributeNames": {"#aliased": word},
                   "ExpressionAttributeValues": {":v": {"S": "X"}}}),
        )
        .await;
    }
}

#[tokio::test]
async fn a_path_with_no_parent_is_refused_with_the_fixed_sentence() {
    let router = emulator();
    create_active_table(&router, "Paths", &[("pk", "HASH", "S")]).await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Paths", "Item": {
            "pk": {"S": "one"},
            "m": {"M": {"kept": {"S": "x"}}},
        }}),
    )
    .await;

    // AWS's text for this condition is a CONSTANT: every row — a missing top-level
    // parent, a missing nested parent, a step through a value that is not a container,
    // a step through an aliased name — states exactly it, and none of them appends the
    // path it refused.
    let fixed = "The document path provided in the update expression is invalid for update";
    let rows: Vec<(&str, Value)> = vec![
        (
            "a map entry under a parent that is not there",
            json!({"UpdateExpression": "SET parent.child = :v"}),
        ),
        (
            "a map entry two levels under a parent that is not there",
            json!({"UpdateExpression": "SET parent.child.leaf = :v"}),
        ),
        (
            "a map entry under a stored map's absent entry",
            json!({"UpdateExpression": "SET m.absent.leaf = :v"}),
        ),
        (
            "a list element under a parent that is not there",
            json!({"UpdateExpression": "SET parent[0] = :v"}),
        ),
        (
            "a map entry under an aliased absent parent",
            json!({"UpdateExpression": "SET #parent.child = :v",
                   "ExpressionAttributeNames": {"#parent": "absent"}}),
        ),
    ];
    for (case, extra) in rows {
        let mut request = json!({
            "TableName": "Paths",
            "Key": {"pk": {"S": "one"}},
            "ExpressionAttributeValues": {":v": {"S": "X"}},
        });
        for (member, value) in extra.as_object().expect("the extra members") {
            request[member] = value.clone();
        }
        let stated = refusal(&router, case, "UpdateItem", request, "ValidationException").await;
        assert_eq!(
            stated, fixed,
            "case {case:?}: expected the fixed sentence with nothing appended, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn every_clause_of_one_update_resolves_against_the_pre_image() {
    let router = emulator();
    create_active_table(&router, "PreImage", &[("pk", "HASH", "S")]).await;

    /// The list every row starts from.
    fn stored() -> Value {
        json!({"L": [{"S": "a"}, {"S": "b"}, {"S": "c"}, {"S": "d"}, {"S": "e"}]})
    }

    // Each row: the case, the expression, what the list READS afterwards, and what
    // `UPDATED_NEW` reports. The rows span the clause combinations the rule generalises
    // over — a removal before and after the index another clause names, several
    // removals in one clause, a removal at the front and at the back, and a write past
    // the end of the list.
    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a removal before the index a write names",
            "REMOVE l[0] SET l[1] = :v",
            json!({"L": [{"S": "X"}, {"S": "c"}, {"S": "d"}, {"S": "e"}]}),
            json!({"l": {"L": [{"S": "X"}, {"S": "c"}]}}),
        ),
        (
            "a removal after the index a write names",
            "REMOVE l[3] SET l[1] = :v",
            json!({"L": [{"S": "a"}, {"S": "X"}, {"S": "c"}, {"S": "e"}]}),
            json!({"l": {"L": [{"S": "X"}, {"S": "e"}]}}),
        ),
        (
            "two removals in one clause",
            "REMOVE l[1], l[2]",
            json!({"L": [{"S": "a"}, {"S": "d"}, {"S": "e"}]}),
            json!({"l": {"L": [{"S": "d"}, {"S": "e"}]}}),
        ),
        (
            "three removals spanning the list",
            "REMOVE l[0], l[2], l[4]",
            json!({"L": [{"S": "b"}, {"S": "d"}]}),
            json!({"l": {"L": [{"S": "b"}]}}),
        ),
        (
            "a removal at the back",
            "REMOVE l[4] SET l[0] = :v",
            json!({"L": [{"S": "X"}, {"S": "b"}, {"S": "c"}, {"S": "d"}]}),
            json!({"l": {"L": [{"S": "X"}]}}),
        ),
    ];
    for (case, expression, after, reported) in rows {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "PreImage",
                   "Item": {"pk": {"S": case}, "l": stored()}}),
        )
        .await;
        // The row whose clauses are all removals references no value token, and a token
        // no expression references makes the request invalid, so the member is sent only
        // when the expression uses it.
        let mut update = json!({"TableName": "PreImage", "Key": {"pk": {"S": case}},
                                "UpdateExpression": expression,
                                "ReturnValues": "UPDATED_NEW"});
        if expression.contains(":v") {
            update["ExpressionAttributeValues"] = json!({":v": {"S": "X"}});
        }
        let updated = ok(&router, "UpdateItem", update).await;
        assert_eq!(
            updated["Attributes"], reported,
            "case {case:?}: expected UPDATED_NEW to report {reported}, got {}",
            updated["Attributes"]
        );
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "PreImage", "Key": {"pk": {"S": case}}}),
        )
        .await;
        assert_eq!(
            read["Item"]["l"], after,
            "case {case:?}: expected the stored list to read {after}, got {}",
            read["Item"]["l"]
        );
    }

    // A write past the end of a list APPENDS. At the index just past the last element
    // that IS where the expression named, so the write is reported; any further index
    // names a location the appended element did not land in, and `UPDATED_NEW` reports
    // no attribute at all.
    for index in [5usize, 6, 10, 99] {
        let case = format!("a write at index {index}, past the end");
        ok(
            &router,
            "PutItem",
            json!({"TableName": "PreImage", "Item": {"pk": {"S": &case}, "l": stored()}}),
        )
        .await;
        let updated = ok(
            &router,
            "UpdateItem",
            json!({"TableName": "PreImage", "Key": {"pk": {"S": &case}},
                   "UpdateExpression": format!("SET l[{index}] = :v"),
                   "ExpressionAttributeValues": {":v": {"S": "X"}},
                   "ReturnValues": "UPDATED_NEW"}),
        )
        .await;
        // The stored list holds five elements, so index 5 is the position the append
        // lands in and every larger index is not.
        let lands_where_named = index == 5;
        let reported = if lands_where_named {
            Some(json!({"l": {"L": [{"S": "X"}]}}))
        } else {
            None
        };
        assert_eq!(
            updated.get("Attributes").cloned(),
            reported,
            "case {case:?}: expected UPDATED_NEW to report {reported:?}, got {updated}"
        );
        let read = ok(
            &router,
            "GetItem",
            json!({"TableName": "PreImage", "Key": {"pk": {"S": &case}}}),
        )
        .await;
        assert_eq!(
            read["Item"]["l"],
            json!({"L": [{"S": "a"}, {"S": "b"}, {"S": "c"}, {"S": "d"}, {"S": "e"},
                         {"S": "X"}]}),
            "case {case:?}: the element is appended to the end, got {}",
            read["Item"]["l"]
        );
    }
}

#[tokio::test]
async fn size_of_a_string_counts_utf8_bytes() {
    let router = emulator();
    create_active_table(&router, "Sizes", &[("pk", "HASH", "S")]).await;

    // One row per byte width a UTF-8 character can take, plus a mixture and the empty
    // string: `(the string, its bytes, its characters)`. `size` "returns the length of
    // the string in bytes" (the `size` function's own documentation), so the rows where
    // the two counts differ are what tells the rule apart from counting characters.
    let strings: Vec<(&str, i64, i64)> = vec![
        ("", 0, 0),
        ("ascii", 5, 5),
        ("é", 2, 1),
        ("日本語", 9, 3),
        ("a\u{00e9}\u{65e5}\u{1f600}", 1 + 2 + 3 + 4, 4),
        ("\u{1f600}\u{1f600}", 8, 2),
    ];
    for (text, bytes, characters) in strings {
        let key = format!("len-{bytes}");
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Sizes",
                   "Item": {"pk": {"S": &key}, "s": {"S": text}}}),
        )
        .await;
        // A conditional write gated on the BYTE count lands.
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Sizes",
                   "Item": {"pk": {"S": &key}, "s": {"S": text}, "ok": {"BOOL": true}},
                   "ConditionExpression": "size(s) = :n",
                   "ExpressionAttributeValues": {":n": {"N": bytes.to_string()}}}),
        )
        .await;
        // The same write gated on the CHARACTER count lands only where the two agree.
        let (status, body) = call(
            &router,
            "PutItem",
            json!({"TableName": "Sizes",
                   "Item": {"pk": {"S": &key}, "s": {"S": text}},
                   "ConditionExpression": "size(s) = :n",
                   "ExpressionAttributeValues": {":n": {"N": characters.to_string()}}}),
        )
        .await;
        let expected_status = if bytes == characters { 200 } else { 400 };
        assert_eq!(
            status, expected_status,
            "case {text:?} ({bytes} bytes, {characters} characters): expected HTTP \
             {expected_status} for a condition on the character count, got {status} {body}"
        );
        // And the inequalities read the byte count too.
        for (comparison, holds) in [
            (format!("size(s) < :{}", "n"), false),
            ("size(s) <= :n".to_string(), true),
            ("size(s) > :n".to_string(), false),
            ("size(s) >= :n".to_string(), true),
        ] {
            let (status, body) = call(
                &router,
                "PutItem",
                json!({"TableName": "Sizes",
                       "Item": {"pk": {"S": &key}, "s": {"S": text}},
                       "ConditionExpression": comparison,
                       "ExpressionAttributeValues": {":n": {"N": bytes.to_string()}}}),
            )
            .await;
            let expected_status = if holds { 200 } else { 400 };
            assert_eq!(
                status, expected_status,
                "case {text:?} {comparison:?} against its {bytes} bytes: expected HTTP \
                 {expected_status}, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_refusal_names_its_shape_id_and_the_class_the_caller_can_act_on() {
    let router = emulator();
    create_active_table(&router, "Classes", &[("pk", "HASH", "S")]).await;

    // The wire `__type` of a MODELED error is the API-version-qualified shape id, so a
    // client that classifies an error by the body alone reads the whole id.
    let (status, body) = call(
        &router,
        "GetItem",
        json!({"TableName": "Absent", "Key": {"pk": {"S": "one"}}}),
    )
    .await;
    assert_eq!(
        (status, body["__type"].as_str()),
        (
            400,
            Some("com.amazonaws.dynamodb.v20120810#ResourceNotFoundException")
        ),
        "case a read of an absent table: expected the qualified shape id, got {status} {body}"
    );

    // One row per refusal whose CLASS says what the caller can do: a parameter the
    // caller must change is a ValidationException, and a limit on the work ONE call may
    // ask for is the control plane's LimitExceededException.
    let tags: Vec<Value> = (0..51)
        .map(|index| json!({"Key": format!("k{index}"), "Value": "v"}))
        .collect();
    let arn = format!("arn:aws:dynamodb:{REGION}:000000000000:table/Classes");
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a tag set past the per-resource ceiling",
            "TagResource",
            json!({"ResourceArn": arn, "Tags": tags}),
            "ValidationException",
        ),
        (
            "two indexes to create in one call",
            "UpdateTable",
            json!({
                "TableName": "Classes",
                "AttributeDefinitions": [
                    {"AttributeName": "a", "AttributeType": "S"},
                    {"AttributeName": "b", "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexUpdates": [
                    {"Create": {"IndexName": "first",
                                "KeySchema": [{"AttributeName": "a", "KeyType": "HASH"}],
                                "Projection": {"ProjectionType": "ALL"}}},
                    {"Create": {"IndexName": "second",
                                "KeySchema": [{"AttributeName": "b", "KeyType": "HASH"}],
                                "Projection": {"ProjectionType": "ALL"}}},
                ],
            }),
            "LimitExceededException",
        ),
        (
            "a stream asked to be disabled on a table that has none",
            "UpdateTable",
            json!({"TableName": "Classes",
                   "StreamSpecification": {"StreamEnabled": false}}),
            "ValidationException",
        ),
        (
            "a ReturnValues outside the set the service accepts",
            "PutItem",
            json!({"TableName": "Classes", "Item": {"pk": {"S": "one"}},
                   "ReturnValues": "BOGUS"}),
            "ValidationException",
        ),
        (
            "secondary indexes projecting more names than the budget",
            "CreateTable",
            json!({
                "TableName": "Projected",
                "BillingMode": "PAY_PER_REQUEST",
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": "gsi", "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexes": (0..6).map(|index| json!({
                    "IndexName": format!("idx{index}"),
                    "KeySchema": [{"AttributeName": "gsi", "KeyType": "HASH"}],
                    "Projection": {
                        "ProjectionType": "INCLUDE",
                        "NonKeyAttributes": (0..20)
                            .map(|name| format!("a{name}"))
                            .collect::<Vec<String>>(),
                    },
                })).collect::<Vec<Value>>(),
            }),
            "ValidationException",
        ),
    ];
    for (case, operation, request, expected_code) in rows {
        refusal(&router, case, operation, request, expected_code).await;
    }

    // The `ReturnValues` refusal states the requirement ALONE, over the set the
    // SERVICE's own check names; every other enumerated member keeps the generic
    // validator's clause, with the rejected value and the member path echoed.
    let stated = refusal(
        &router,
        "a ReturnValues outside the set",
        "PutItem",
        json!({"TableName": "Classes", "Item": {"pk": {"S": "one"}},
               "ReturnValues": "BOGUS"}),
        "ValidationException",
    )
    .await;
    assert_eq!(
        stated,
        "1 validation error detected: Failed to satisfy constraint: Member must satisfy enum \
         value set: [ALL_OLD, UPDATED_OLD, ALL_NEW, UPDATE_NEW, NONE]",
        "case a ReturnValues outside the set: got {stated:?}"
    );
    let stated = refusal(
        &router,
        "another enumerated member outside its set",
        "GetItem",
        json!({"TableName": "Classes", "Key": {"pk": {"S": "one"}},
               "ReturnConsumedCapacity": "NOPE"}),
        "ValidationException",
    )
    .await;
    assert_eq!(
        stated,
        "1 validation error detected: Value 'NOPE' at 'returnConsumedCapacity' failed to satisfy \
         constraint: Member must satisfy enum value set: [INDEXES, TOTAL, NONE]",
        "case another enumerated member outside its set: got {stated:?}"
    );

    // The projection budget is a BOUND, so a table exactly at it is accepted: five
    // indexes of twenty names each is the documented 100.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "AtTheBudget",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "gsi", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": (0..5).map(|index| json!({
                "IndexName": format!("idx{index}"),
                "KeySchema": [{"AttributeName": "gsi", "KeyType": "HASH"}],
                "Projection": {
                    "ProjectionType": "INCLUDE",
                    "NonKeyAttributes": (0..20)
                        .map(|name| format!("a{name}"))
                        .collect::<Vec<String>>(),
                },
            })).collect::<Vec<Value>>(),
        }),
    )
    .await;
}

#[tokio::test]
async fn a_key_condition_names_the_schema_element_or_the_operator_it_broke() {
    let router = emulator();
    create_active_table(
        &router,
        "Keys",
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
    )
    .await;

    // Each row: the case, the Query request, and the WHOLE message. The rows span the
    // two ways a key condition can be wrong — it does not CONSTRAIN a key schema
    // element it has to (named, so the caller knows which), or it uses a comparison the
    // member does not take (AWS's fixed sentence, which names no part of the request) —
    // over both key schema elements and over several operators.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a second condition on an attribute that is not a key",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk = :p AND extra = :e",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":e": {"S": "x"}}}),
            "Query condition missed key schema element: sk",
        ),
        (
            "a condition that constrains only the sort key",
            json!({"TableName": "Keys", "KeyConditionExpression": "sk = :s",
                   "ExpressionAttributeValues": {":s": {"N": "1"}}}),
            "Query condition missed key schema element: pk",
        ),
        (
            "begins_with on the partition key",
            json!({"TableName": "Keys", "KeyConditionExpression": "begins_with(pk, :p)",
                   "ExpressionAttributeValues": {":p": {"S": "a"}}}),
            "Query key condition not supported",
        ),
        (
            "a range comparison on the partition key",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk > :p AND sk = :s",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":s": {"N": "1"}}}),
            "Query key condition not supported",
        ),
        (
            "not-equal on the sort key",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk = :p AND sk <> :s",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":s": {"N": "1"}}}),
            "Invalid KeyConditionExpression: Invalid operator used in KeyConditionExpression",
        ),
        (
            "two conditions joined by OR",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk = :p OR sk = :s",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":s": {"N": "1"}}}),
            "Invalid KeyConditionExpression: Invalid operator used in KeyConditionExpression",
        ),
        (
            "a condition function the member does not take",
            json!({"TableName": "Keys",
                   "KeyConditionExpression": "pk = :p AND contains(sk, :s)",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":s": {"N": "1"}}}),
            "Invalid KeyConditionExpression: Invalid operator used in KeyConditionExpression",
        ),
        (
            "begins_with on a numeric sort key",
            json!({"TableName": "Keys",
                   "KeyConditionExpression": "pk = :p AND begins_with(sk, :s)",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":s": {"N": "1"}}}),
            "Invalid KeyConditionExpression: Incorrect operand type for operator or function; \
             operator or function: begins_with, operand type: N",
        ),
        (
            "the legacy map and the expression in one request",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "a"}},
                   "KeyConditions": {"pk": {"ComparisonOperator": "EQ",
                                            "AttributeValueList": [{"S": "a"}]}}}),
            "Can not use both expression and non-expression parameters in the same request: \
             Non-expression parameters: {KeyConditions} Expression parameters: \
             {KeyConditionExpression}",
        ),
        (
            "a key condition that stops at the end of the expression",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk =",
                   "ExpressionAttributeValues": {":p": {"S": "a"}}}),
            "Invalid KeyConditionExpression: Syntax error; token: \"<EOF>\", near: \"=\"",
        ),
        (
            // A syntax error names the token the parse STOPPED at, not the one after it.
            "a key condition that stops at a token in the middle",
            json!({"TableName": "Keys", "KeyConditionExpression": "pk = :p AND , sk = :s",
                   "ExpressionAttributeValues": {":p": {"S": "a"}, ":s": {"N": "1"}}}),
            "Invalid KeyConditionExpression: Syntax error; token: \",\", near: \"AND , sk\"",
        ),
    ];
    for (case, request, expected) in cases {
        let stated = refusal(&router, case, "Query", request, "ValidationException").await;
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }

    // The partition key alone, and the partition key with each comparison the member
    // DOES take on the sort key, are served: the refusals above are the rule and not a
    // blanket refusal of the member.
    for condition in [
        "pk = :p",
        "pk = :p AND sk = :s",
        "pk = :p AND sk < :s",
        "pk = :p AND sk >= :s",
        "pk = :p AND sk BETWEEN :s AND :s",
    ] {
        // Only the tokens this condition REFERENCES are defined: an
        // `ExpressionAttributeValues` entry no expression uses makes the request
        // invalid, so the one-term condition sends only its own.
        let values: serde_json::Map<String, Value> =
            [(":p", json!({"S": "a"})), (":s", json!({"N": "1"}))]
                .into_iter()
                .filter(|(token, _)| condition.contains(token))
                .map(|(token, value)| (token.to_string(), value))
                .collect();
        ok(
            &router,
            "Query",
            json!({"TableName": "Keys", "KeyConditionExpression": condition,
                   "ExpressionAttributeValues": values}),
        )
        .await;
    }
}

#[tokio::test]
async fn a_table_is_addressed_by_its_arn_wherever_a_name_does() {
    let router = emulator();
    create_active_table(&router, "Addressed", &[("pk", "HASH", "S")]).await;
    let arn = format!("arn:aws:dynamodb:{REGION}:000000000000:table/Addressed");
    ok(
        &router,
        "PutItem",
        json!({"TableName": arn, "Item": {"pk": {"S": "one"}}}),
    )
    .await;
    let read = ok(
        &router,
        "GetItem",
        json!({"TableName": arn, "Key": {"pk": {"S": "one"}}}),
    )
    .await;
    assert_eq!(
        read["Item"]["pk"],
        json!({"S": "one"}),
        "case a read addressed by the table's ARN: expected the stored item, got {read}"
    );

    // A value spelled as an ARN that is NOT a well-formed ARN is refused by the ARN
    // FORM: the rows span each field that can be empty and a field count short of one.
    for case in [
        format!("arn::dynamodb:{REGION}:000000000000:table/Addressed"),
        format!("arn:aws::{REGION}:000000000000:table/Addressed"),
        "arn:aws:dynamodb::000000000000:table/Addressed".to_string(),
        format!("arn:aws:dynamodb:{REGION}::table/Addressed"),
        format!("arn:aws:dynamodb:{REGION}:000000000000:"),
        format!("arn:aws:dynamodb:{REGION}:000000000000"),
    ] {
        let stated = refusal(
            &router,
            &case,
            "GetItem",
            json!({"TableName": case, "Key": {"pk": {"S": "one"}}}),
            "ValidationException",
        )
        .await;
        assert!(
            stated.contains("Valid ARN format is 'arn:"),
            "case {case:?}: expected the refusal to state the ARN form, got {stated:?}"
        );
    }

    // A WELL-FORMED ARN of ANOTHER service is no way of naming a table at all, so it is
    // refused as the table NAME it would have to be.
    for case in [format!(
        "arn:aws:kinesis:{REGION}:000000000000:stream/Addressed"
    )] {
        let stated = refusal(
            &router,
            &case,
            "GetItem",
            json!({"TableName": case, "Key": {"pk": {"S": "one"}}}),
            "ValidationException",
        )
        .await;
        assert!(
            stated.contains("Member must satisfy regular expression pattern:"),
            "case {case:?}: expected the table-name pattern, got {stated:?}"
        );
    }

    // An ARN of THIS service whose resource is not a table addresses some other kind of
    // resource: what is wrong with it is the resource TYPE, which is what the refusal
    // states, over the whole value.
    for case in [
        format!("arn:aws:dynamodb:{REGION}:000000000000:backup/Addressed"),
        format!("arn:aws:dynamodb:{REGION}:000000000000:table/Addressed/index/idx"),
    ] {
        let stated = refusal(
            &router,
            &case,
            "GetItem",
            json!({"TableName": case, "Key": {"pk": {"S": "one"}}}),
            "ValidationException",
        )
        .await;
        assert_eq!(
            stated,
            format!("1 validation error detected: Invalid resource type in '{case}'"),
            "case {case:?}: expected the resource-type sentence, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_states_what_stands_in_its_way() {
    let router = emulator();
    create_active_table(&router, "Protected", &[("pk", "HASH", "S")]).await;
    create_active_table(&router, "Indexed", &[("pk", "HASH", "S")]).await;

    // Deletion protection: AWS's text is a CONSTANT, with nothing of the table appended.
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": "Protected", "DeletionProtectionEnabled": true}),
    )
    .await;
    let stated = refusal(
        &router,
        "a delete of a protected table",
        "DeleteTable",
        json!({"TableName": "Protected"}),
        "ValidationException",
    )
    .await;
    assert_eq!(
        stated,
        "Resource cannot be deleted as it is currently protected against deletion. Disable \
         deletion protection first.",
        "case a delete of a protected table: got {stated:?}"
    );
    // Clearing the flag is what makes the delete possible, so the refusal is the flag's
    // and not the table's.
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": "Protected", "DeletionProtectionEnabled": false}),
    )
    .await;
    ok(&router, "DeleteTable", json!({"TableName": "Protected"})).await;

    // An index of the table in a non-ACTIVE status is the case AWS states on its own.
    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Indexed",
            "AttributeDefinitions": [{"AttributeName": "gsi", "AttributeType": "S"}],
            "GlobalSecondaryIndexUpdates": [
                {"Create": {"IndexName": "idx",
                            "KeySchema": [{"AttributeName": "gsi", "KeyType": "HASH"}],
                            "Projection": {"ProjectionType": "ALL"}}},
            ],
        }),
    )
    .await;
    let stated = refusal(
        &router,
        "a delete while an index is being created",
        "DeleteTable",
        json!({"TableName": "Indexed"}),
        "ResourceInUseException",
    )
    .await;
    assert_eq!(
        stated,
        "Attempt to change a resource which is still in use: Cannot delete table while indexes \
         are being created, updated, or deleted.",
        "case a delete while an index is being created: got {stated:?}"
    );
}

#[tokio::test]
async fn a_partiql_read_is_held_to_the_index_and_the_statement_its_token_came_from() {
    let router = emulator();
    // One table carrying both kinds of secondary index, which is what tells the
    // consistency rule apart: a GLOBAL index has its own partitions, a LOCAL one shares
    // the table's.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Indexes",
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                          {"AttributeName": "sk", "KeyType": "RANGE"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "gsi", "AttributeType": "S"},
                {"AttributeName": "lsi", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [
                {"IndexName": "by-global",
                 "KeySchema": [{"AttributeName": "gsi", "KeyType": "HASH"}],
                 "Projection": {"ProjectionType": "ALL"}},
            ],
            "LocalSecondaryIndexes": [
                {"IndexName": "by-local",
                 "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                               {"AttributeName": "lsi", "KeyType": "RANGE"}],
                 "Projection": {"ProjectionType": "ALL"}},
            ],
        }),
    )
    .await;
    std::thread::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    );
    for sort in ["s1", "s2"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Indexes", "Item": {
                "pk": {"S": "p1"}, "sk": {"S": sort},
                "gsi": {"S": "g1"}, "lsi": {"S": sort},
            }}),
        )
        .await;
    }

    // A strongly consistent read of a GLOBAL index is refused; of a LOCAL one, served.
    let stated = refusal(
        &router,
        "a consistent read of a global secondary index",
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM \"Indexes\".\"by-global\" WHERE gsi = 'g1'",
               "ConsistentRead": true}),
        "ValidationException",
    )
    .await;
    assert_eq!(
        stated, "Strongly consistent read is not supported on Global Secondary Indexes",
        "case a consistent read of a global secondary index: got {stated:?}"
    );
    for consistent in [false, true] {
        ok(
            &router,
            "ExecuteStatement",
            json!({"Statement": "SELECT * FROM \"Indexes\".\"by-local\" WHERE pk = 'p1'",
                   "ConsistentRead": consistent}),
        )
        .await;
    }
    ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM \"Indexes\".\"by-global\" WHERE gsi = 'g1'"}),
    )
    .await;

    // A `NextToken` continues ONE statement's result set: the statement that issued it
    // resumes from it, and any other statement is refused.
    let first = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM \"Indexes\" WHERE pk = 'p1'", "Limit": 1}),
    )
    .await;
    let token = first["NextToken"]
        .as_str()
        .expect("case a truncated page: it reports a NextToken")
        .to_string();
    let resumed = ok(
        &router,
        "ExecuteStatement",
        json!({"Statement": "SELECT * FROM \"Indexes\" WHERE pk = 'p1'", "Limit": 1,
               "NextToken": token}),
    )
    .await;
    assert_ne!(
        resumed["Items"], first["Items"],
        "case the issuing statement resumes: expected the next page, got {resumed}"
    );
    for foreign in [
        "SELECT * FROM \"Indexes\" WHERE pk = 'p1' ",
        "SELECT * FROM \"Indexes\"",
        "SELECT pk FROM \"Indexes\" WHERE pk = 'p1'",
    ] {
        let stated = refusal(
            &router,
            foreign,
            "ExecuteStatement",
            json!({"Statement": foreign, "Limit": 1, "NextToken": token}),
            "ValidationException",
        )
        .await;
        assert_eq!(
            stated, "NextToken does not match request",
            "case a token presented to {foreign:?}: got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_policy_that_would_lock_the_caller_out_needs_the_confirmation() {
    let router = emulator();
    create_active_table(&router, "Locked", &[("pk", "HASH", "S")]).await;
    let arn = format!("arn:aws:dynamodb:{REGION}:000000000000:table/Locked");

    // One row per way a document can take the caller's own ability to change the policy
    // away: each of the three policy-management actions by name, and the wildcards that
    // cover them — each stated against every spelling a `Principal` element names THIS
    // account by, because a statement that denies the account denies each of its
    // principals: the wildcard, the account's root user, and the bare account id that
    // stands for that root.
    let principals = [
        json!("*"),
        json!({"AWS": "arn:aws:iam::000000000000:root"}),
        json!({"AWS": "000000000000"}),
    ];
    for action in [
        "dynamodb:PutResourcePolicy",
        "dynamodb:DeleteResourcePolicy",
        "dynamodb:GetResourcePolicy",
        "dynamodb:*",
        "*",
    ] {
        for principal in &principals {
            let document = json!({
                "Version": "2012-10-17",
                "Statement": [{"Effect": "Deny", "Principal": principal, "Action": action,
                               "Resource": arn}],
            })
            .to_string();
            let stated = refusal(
                &router,
                action,
                "PutResourcePolicy",
                json!({"ResourceArn": arn, "Policy": document}),
                "AccessDeniedException",
            )
            .await;
            assert_eq!(
                stated,
                "The new resource policy will not allow you to update the resource policy in \
                 the future.",
                "case a Deny of {action:?} for {principal}: got {stated:?}"
            );
            // Nothing was attached, so the next attempt starts from no policy at all —
            // which is what conditioning the accepted attach below on the ABSENCE of a
            // policy holds it to ("to conditionally attach a policy when no policy exists
            // for the resource, specify NO_POLICY for the revision ID"). The stored state
            // is what that guard reads; a `GetResourcePolicy` of its own would answer
            // from before this row's writes, because that read is eventually consistent.
            //
            // The same document WITH the confirmation is accepted: the member is what
            // makes the choice deliberate, not something the service refuses outright.
            ok(
                &router,
                "PutResourcePolicy",
                json!({"ResourceArn": arn, "Policy": document,
                       "ConfirmRemoveSelfResourceAccess": true,
                       "ExpectedRevisionId": "NO_POLICY"}),
            )
            .await;
            ok(&router, "DeleteResourcePolicy", json!({"ResourceArn": arn})).await;
        }
    }

    // A document that denies something ELSE leaves the caller's own access alone, so it
    // needs no confirmation.
    for action in ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:Query"] {
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{"Effect": "Deny", "Principal": "*", "Action": action,
                           "Resource": arn}],
        })
        .to_string();
        ok(
            &router,
            "PutResourcePolicy",
            json!({"ResourceArn": arn, "Policy": document}),
        )
        .await;
    }
}
