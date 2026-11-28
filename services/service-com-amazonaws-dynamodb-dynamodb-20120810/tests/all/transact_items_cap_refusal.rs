// SPDX-License-Identifier: CC0-1.0

//! What a TRANSACTION whose `TransactItems` list is past the 100-action cap is refused
//! with, driven end to end.
//!
//! RULE under test: a list member carrying more elements than the `@length` maximum the
//! model declares for it is refused 400 `ValidationException` whose message is the
//! framework's constraint envelope over ONE clause that states the member path, as the
//! operation that was served names it, and the bound that was broken. The transactional
//! READ's clause echoes nothing of the rejected list; the transactional WRITE's QUOTES the
//! actions it was sent, in the object rendering this service states a refused structure
//! value in (`[TransactWriteItem(conditionCheck=null, put=Put(item={<attribute>=
//! AttributeValue(…)}, tableName=<table>, …), delete=null, update=null), …]`) — every
//! structure member spelled out, every member the request left out rendered unset, and every
//! value in it the request's own. Both transaction operations declare that bound on the same
//! member (`smithy_query(query="operation",
//! operation="TransactGetItems,TransactWriteItems")`: `TransactItems` is `@required`, and
//! `smithy_query(query="shape", shape="com.amazonaws.dynamodb#TransactGetItemList")` and
//! `TransactWriteItemList` are both `@length{min:1,max:100}`), so both state the same member
//! and the same bound, and neither writes anything.
//!
//! The rows differ along what the rule generalises over: the operation (the transactional
//! read and the transactional write), the ACTION its elements state (`Get`, `Put`,
//! `Delete`, `Update`, `ConditionCheck`, and a list mixing two of them), how far past the
//! bound the list went (one past it, half again as many, and twice the bound), how many
//! tables the actions name, and what the items carry (a bare key, and attributes of
//! several datatypes). The bound itself is driven AT the bound, where the call is served,
//! and one past it.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<Operation>`) through the same `MultiServiceRouter` the
//! emulator's front doors use.
//!
//! Every call goes over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<Operation>`) through the same `MultiServiceRouter` the
//! emulator's front doors use.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The tables the rows address, each keyed on one string attribute.
const TABLE: &str = "TransactCap";

/// The second table, so a list naming two tables is refused the same way.
const OTHER: &str = "TransactCapOther";

/// That key attribute.
const KEY: &str = "pk";

/// The cap both transactional lists declare.
const CAP: usize = 100;

/// The whole message a `TransactItems` list past that cap earns on the transactional READ,
/// whose clause echoes nothing of the list.
fn expected_message() -> String {
    format!(
        "1 validation error detected: Value at 'TransactItems' failed to satisfy constraint: \
         Member must have length less than or equal to {CAP}"
    )
}

/// How the WRITE's clause ends: the member and the bound, after the quoted list.
fn write_clause_tail() -> String {
    format!(
        "' at 'TransactItems' failed to satisfy constraint: Member must have length less than \
         or equal to {CAP}"
    )
}

/// How the WRITE's clause opens, with the first TWO actions quoted: the envelope, the open
/// list, the rendering of element 0 and of element 1 — so the separator between elements and
/// the per-element values are both what the rows are held to.
fn write_clause_head(action: Action) -> String {
    format!(
        "1 validation error detected: Value '[{}, {}",
        action.quoted(0),
        action.quoted(1)
    )
}

/// The fields of the attribute-value object this service's own request validator renders a
/// value with, in the order it states them — written out here rather than read off the crate
/// under test, so the RENDERING is what these rows are held to.
const CORAL_FIELDS: [&str; 16] = [
    "s", "n", "b", "iNT", "dECIMAL", "dOUBLE", "sS", "nS", "bS", "iNTSET", "dECIMALSET",
    "dOUBLESET", "m", "l", "nULL", "bOOL",
];

/// One value in that frame: the field it carries filled from the request, every other field
/// rendered unset.
fn attribute_value(field: &str, value: &str) -> String {
    let fields: Vec<String> = CORAL_FIELDS
        .iter()
        .map(|declared| {
            if *declared == field {
                format!("{declared}={value}")
            } else {
                format!("{declared}=null")
            }
        })
        .collect();
    format!("AttributeValue({})", fields.join(", "))
}

/// The fields every quoted action ends with: the two substitution tables, what it asks for on
/// a failed condition, and the workload-profile field no request can set.
const TRAILING_FIELDS: &str = "expressionAttributeNames=null, expressionAttributeValues=null, \
                               returnValuesOnConditionCheckFailure=null, \
                               workloadProfileName=null";

/// The key of element `index` as the rendering quotes it.
fn quoted_key(index: usize) -> String {
    format!(
        "{{{KEY}={}}}",
        attribute_value("s", &format!("k{index}"))
    )
}

/// The item of element `index` as the rendering quotes it: the attributes in NAME order,
/// each in the attribute-value frame, the nested map's own value in that frame too.
fn quoted_item(index: usize) -> String {
    let flag = if index.is_multiple_of(2) { "true" } else { "false" };
    format!(
        "{{doc={}, flag={}, note={}, {KEY}={}, tags={}, tally={}}}",
        attribute_value("m", &format!("{{leaf={}}}", attribute_value("s", "deep"))),
        attribute_value("bOOL", flag),
        attribute_value("s", &format!("note {index}")),
        attribute_value("s", &format!("k{index}")),
        attribute_value("sS", "[a, b]"),
        attribute_value("n", &index.to_string()),
    )
}

/// The key of element `index` of this row's table, as a `Key` map.
fn key(index: usize) -> Value {
    json!({KEY: {"S": format!("k{index}")}})
}

/// The table element `index` names: every third one addresses the second table, so a list
/// spanning two tables is refused by the same clause.
fn table_of(index: usize) -> &'static str {
    if index.is_multiple_of(3) {
        OTHER
    } else {
        TABLE
    }
}

/// An item carrying the key of `index` and attributes of several datatypes, so what the
/// elements carry cannot reach the message.
fn item(index: usize) -> Value {
    json!({
        KEY: {"S": format!("k{index}")},
        "note": {"S": format!("note {index}")},
        "tally": {"N": index.to_string()},
        "flag": {"BOOL": index.is_multiple_of(2)},
        "tags": {"SS": ["a", "b"]},
        "doc": {"M": {"leaf": {"S": "deep"}}},
    })
}

/// The action kinds a row's elements state, each as it builds element `index`.
#[derive(Clone, Copy)]
enum Action {
    Get,
    Put,
    Delete,
    Update,
    ConditionCheck,
    /// A `Put` at an even position and a `Delete` at an odd one.
    PutAndDelete,
}

impl Action {
    /// Element `index` of a list of this kind.
    fn element(self, index: usize) -> Value {
        let table = table_of(index);
        match self {
            Self::Get => json!({"Get": {"TableName": table, "Key": key(index)}}),
            Self::Put => json!({"Put": {"TableName": table, "Item": item(index)}}),
            Self::Delete => json!({"Delete": {"TableName": table, "Key": key(index)}}),
            Self::Update => json!({"Update": {
                "TableName": table,
                "Key": key(index),
                "UpdateExpression": "SET tally = :n",
                "ExpressionAttributeValues": {":n": {"N": index.to_string()}},
            }}),
            Self::ConditionCheck => json!({"ConditionCheck": {
                "TableName": table,
                "Key": key(index),
                "ConditionExpression": "attribute_not_exists(note)",
            }}),
            Self::PutAndDelete => {
                if index.is_multiple_of(2) {
                    Self::Put.element(index)
                } else {
                    Self::Delete.element(index)
                }
            }
        }
    }

    /// `count` elements of this kind.
    fn list(self, count: usize) -> Value {
        Value::Array((0..count).map(|index| self.element(index)).collect())
    }

    /// Element `index` as the WRITE's refusal quotes it: the four action members of one
    /// `TransactWriteItem`, the one this element states rendered and the other three unset.
    fn quoted(self, index: usize) -> String {
        let table = table_of(index);
        let (check, put, delete, update) = match self {
            // A `Get` belongs to the transactional READ, whose clause quotes nothing.
            Self::Get => return String::new(),
            Self::Put => (
                "null".to_string(),
                format!(
                    "Put(item={}, tableName={table}, conditionExpression=null, {TRAILING_FIELDS})",
                    quoted_item(index)
                ),
                "null".to_string(),
                "null".to_string(),
            ),
            Self::Delete => (
                "null".to_string(),
                "null".to_string(),
                format!(
                    "Delete(key={}, tableName={table}, conditionExpression=null, \
                     {TRAILING_FIELDS})",
                    quoted_key(index)
                ),
                "null".to_string(),
            ),
            Self::Update => (
                "null".to_string(),
                "null".to_string(),
                "null".to_string(),
                format!(
                    "Update(key={}, updateExpression=SET tally = :n, tableName={table}, \
                     conditionExpression=null, expressionAttributeNames=null, \
                     expressionAttributeValues={{:n={}}}, \
                     returnValuesOnConditionCheckFailure=null, workloadProfileName=null)",
                    quoted_key(index),
                    attribute_value("n", &index.to_string())
                ),
            ),
            Self::ConditionCheck => (
                format!(
                    "ConditionCheck(key={}, tableName={table}, \
                     conditionExpression=attribute_not_exists(note), {TRAILING_FIELDS})",
                    quoted_key(index)
                ),
                "null".to_string(),
                "null".to_string(),
                "null".to_string(),
            ),
            Self::PutAndDelete => {
                if index.is_multiple_of(2) {
                    return Self::Put.quoted(index);
                }
                return Self::Delete.quoted(index);
            }
        };
        format!(
            "TransactWriteItem(conditionCheck={check}, put={put}, delete={delete}, \
             update={update})"
        )
    }
}

/// The operation a list of actions of this kind belongs to.
fn operation_for(action: Action) -> &'static str {
    match action {
        Action::Get => "TransactGetItems",
        _ => "TransactWriteItems",
    }
}

/// The rows: the case, the action its elements state, and the element count.
fn rows() -> Vec<(String, Action, usize)> {
    let actions: [(&str, Action); 6] = [
        ("gets", Action::Get),
        ("puts", Action::Put),
        ("deletes", Action::Delete),
        ("updates", Action::Update),
        ("condition checks", Action::ConditionCheck),
        ("puts and deletes", Action::PutAndDelete),
    ];
    let counts = [CAP + 1, CAP + CAP / 2, CAP * 2];
    actions
        .into_iter()
        .flat_map(|(named, action)| {
            counts
                .into_iter()
                .map(move |count| (format!("{count} {named}"), action, count))
        })
        .collect()
}

#[tokio::test]
async fn a_transaction_past_the_action_cap_is_refused_with_the_member_and_the_bound() {
    let router = emulator();
    for table in [TABLE, OTHER] {
        create_active_table(&router, table, &[(KEY, "HASH", "S")]).await;
    }

    for (case, action, count) in rows() {
        let operation = operation_for(action);
        let (status, answered) = call(
            &router,
            operation,
            json!({"TransactItems": action.list(count)}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({operation}): expected 400 ValidationException, got {status} with \
             {answered}"
        );
        let stated = message(&answered);
        if operation == "TransactGetItems" {
            let expected = expected_message();
            assert_eq!(
                stated, expected,
                "case {case:?} ({operation}): expected the member and the bound alone, got \
                 {stated:?}"
            );
            continue;
        }
        // The WRITE's clause quotes the actions it was sent: the open list with the first
        // actions rendered in full, and the member and the bound after it. What comes between
        // is the rest of the submitted list, which a very long one is clamped to a ceiling
        // of its own rather than let amplify the response
        // (`crate::transact_write_items_cap_quotes_its_actions` reads a whole such clause
        // back, over a list the ceiling does not reach).
        let head = write_clause_head(action);
        let tail = write_clause_tail();
        assert!(
            stated.starts_with(&head),
            "case {case:?} ({operation}): expected the clause to quote the submitted actions \
             from {head:?}, got {stated:?}"
        );
        assert!(
            stated.ends_with(&tail),
            "case {case:?} ({operation}): expected the clause to end in {tail:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn the_refusal_states_nothing_the_emulator_computed_about_the_list() {
    let router = emulator();
    for table in [TABLE, OTHER] {
        create_active_table(&router, table, &[(KEY, "HASH", "S")]).await;
    }

    // Nothing the emulator computed about the request may reach the message: not the count
    // of elements, not a byte total, and not the operation that refused it. The transactional
    // READ states nothing of the list at all, so for that one not a key, an attribute name, a
    // table name or an expression either.
    let computed = ["101 items", "150 items", " bytes", "emulator", "TransactGetItems"];
    let nothing_of_the_list = [
        "k0",
        "note",
        "tally",
        "TransactCap",
        "AttributeValue",
        "TransactGetItem(",
        "workloadProfileName",
        "attribute_not_exists",
    ];
    for (case, action, count) in rows() {
        let operation = operation_for(action);
        let (_, answered) = call(
            &router,
            operation,
            json!({"TransactItems": action.list(count)}),
        )
        .await;
        let stated = message(&answered);
        let forbidden: Vec<&str> = if operation == "TransactGetItems" {
            computed
                .iter()
                .chain(nothing_of_the_list.iter())
                .copied()
                .collect()
        } else {
            computed.to_vec()
        };
        for leaked in forbidden {
            assert!(
                !stated.contains(leaked),
                "case {case:?} ({operation}): the clause must not state {leaked:?}, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_transaction_writes_nothing_and_the_list_at_the_cap_is_applied() {
    let router = emulator();
    for table in [TABLE, OTHER] {
        create_active_table(&router, table, &[(KEY, "HASH", "S")]).await;
    }

    // One past the cap: refused, and no item of either table was written.
    let (status, answered) = call(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": Action::Put.list(CAP + 1)}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a list past the cap is refused, got {status} with {answered}"
    );
    for table in [TABLE, OTHER] {
        let scanned = ok(
            &router,
            "Scan",
            json!({"TableName": table, "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            scanned["Count"], json!(0),
            "the refused transaction must write nothing to {table}, got {scanned}"
        );
    }

    // AT the cap the transaction is served and applied in full, which is what makes the
    // rows above about the bound and not an off-by-one.
    ok(
        &router,
        "TransactWriteItems",
        json!({"TransactItems": Action::Put.list(CAP)}),
    )
    .await;
    let mut written = 0_i64;
    for table in [TABLE, OTHER] {
        let scanned = ok(
            &router,
            "Scan",
            json!({"TableName": table, "ConsistentRead": true}),
        )
        .await;
        written += scanned["Count"].as_i64().unwrap_or_default();
    }
    assert_eq!(
        written,
        i64::try_from(CAP).unwrap_or_default(),
        "a transaction at the cap must apply every one of its actions, got {written} items"
    );

    // And the transactional READ at the cap is served too, over the items just written.
    let read = ok(
        &router,
        "TransactGetItems",
        json!({"TransactItems": Action::Get.list(CAP)}),
    )
    .await;
    assert_eq!(
        read["Responses"].as_array().map(Vec::len),
        Some(CAP),
        "a read transaction at the cap reports one ItemResponse per element, got {read}"
    );
}
