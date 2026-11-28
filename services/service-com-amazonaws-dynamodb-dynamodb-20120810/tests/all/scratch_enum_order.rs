// SPDX-License-Identifier: CC0-1.0

//! The admissible set a constraint clause ends with, over the surfaces the main
//! enum-order table (`enum_value_set_order`) does not reach.
//!
//! The rule is that one: a refusal of a value outside an enumerated member's set states that
//! set in the order THIS SERVICE states it in, which is a property of the ENUM and not of the
//! operation, the member path or the shape the member sits in. So the same two sets are read
//! back here through other carriers — the table walk's own `Select`, and the comparator of
//! each LEGACY condition map the main table does not carry — and the order must be the same
//! one whichever request carried the value. Both sets are STATED once, in
//! `enum_value_set_order`, and imported here, so neither suite can pin an order the other
//! does not.
//!
//! The rows differ along what the rule generalises over: which enumerated member is broken,
//! which operation carries it, which member path it is reported under, and the FORM of the
//! rejected value (the right value in the wrong case, a misspelling).

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message};
use crate::enum_value_set_order::{envelope_for, COMPARISON_OPERATOR_SET, SELECT_SET};
use crate::table_items::create_active_table;

/// The table the rows are addressed to. It exists and is active before any row runs, so no
/// row is answered by a missing table instead of by the constraint.
const TABLE: &str = "Ordered";

#[tokio::test]
async fn the_admissible_set_is_stated_in_one_order_whichever_request_carried_the_value() {
    let router = emulator();
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;

    // (case, operation, request, the rejected value, the member path, the set)
    type Row = (
        &'static str,
        &'static str,
        Value,
        &'static str,
        &'static str,
        &'static [&'static str],
    );
    let rows: Vec<Row> = vec![
        (
            "the table walk's own Select, in the wrong case",
            "Scan",
            json!({"TableName": TABLE, "Select": "count"}),
            "count",
            "select",
            SELECT_SET,
        ),
        (
            "the comparator of a legacy query filter, misspelled",
            "Query",
            json!({"TableName": TABLE,
                   "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "one"}},
                   "QueryFilter": {"tag": {"ComparisonOperator": "EQUAL",
                                           "AttributeValueList": [{"S": "a"}]}}}),
            "EQUAL",
            "queryFilter.tag.member.comparisonOperator",
            COMPARISON_OPERATOR_SET,
        ),
        (
            "the comparator of a legacy expected entry, in the wrong case",
            "UpdateItem",
            json!({"TableName": TABLE, "Key": {"pk": {"S": "one"}},
                   "AttributeUpdates": {"v": {"Action": "PUT", "Value": {"S": "x"}}},
                   "Expected": {"v": {"ComparisonOperator": "ne",
                                      "AttributeValueList": [{"S": "a"}]}}}),
            "ne",
            "expected.v.member.comparisonOperator",
            COMPARISON_OPERATOR_SET,
        ),
    ];

    for (case, operation, request, value, path, set) in rows {
        let (status, answered) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {answered}"
        );
        let expected = format!(
            "{}Value '{value}' at '{path}' failed to satisfy \
             constraint: Member must satisfy enum value set: [{}]",
            envelope_for(path),
            set.join(", ")
        );
        let stated = message(&answered);
        assert_eq!(
            stated, expected,
            "case {case:?} ({operation}): expected {expected:?}, got {stated:?}"
        );
    }
}
