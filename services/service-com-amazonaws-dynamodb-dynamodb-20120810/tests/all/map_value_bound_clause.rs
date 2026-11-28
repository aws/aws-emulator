// SPDX-License-Identifier: CC0-1.0

//! How this service reports a bound the model declares on a MAP member's VALUES, over the
//! one map input whose values carry one: a batched write's `RequestItems`.
//!
//! ONE RULE, over a table of inputs rather than one recorded request: a constraint broken by
//! the VALUES of a map input member is refused with HTTP 400 `ValidationException` whose
//! message states the clause at the MAP member — not at the entry whose value broke it —
//! quoting the map the request carried and listing the value shape's WHOLE declared
//! requirement set inside AWS's `Map value must satisfy constraint: [ … ]` sentence,
//! maximum first. Which end of the bound the request happened to break does not change the
//! set that is stated, and an entry outside the bound is refused whatever the map's other
//! entries carry.
//!
//! The bound comes from the Smithy model, through `smithy_query(query="shape",
//! shape="com.amazonaws.dynamodb#BatchWriteItemRequestMap")`: its value targets
//! `WriteRequests`, `@length{min:1,max:25}` (the map's own entries carry the same bound,
//! which is a different count and a different clause —
//! `tests/all/collection_cap_constraint_reports.rs` pins that one).
//!
//! The rows differ along what the rule generalises over: which end of the bound was broken
//! (a list of none, and lists past the maximum), how far past it the list went (one past it
//! and half again as many), the shape of the write requests the list carries (puts, deletes
//! and both together), how many entries the map has (one and two), and whether the map's
//! other entry is itself inside the bound. At the bound the request is served, and a map
//! whose every entry is inside the bound but whose operations sum past what one call may
//! carry states the CALL's own fixed sentence instead, which names no member and echoes
//! nothing of the request.

use serde_json::{json, Value};

use crate::emulator::{call, emulator, error_code, message, ok};
use crate::table_items::create_active_table;

/// The bound the model declares for a `RequestItems` value, read off the shape named in the
/// module doc.
const LEAST: usize = 1;
const MOST: usize = 25;

/// The tables the rows address. Two of them, so a row can spread its entries.
const FIRST: &str = "Valued";
const SECOND: &str = "Valued2";

/// The member path the request validator builds for that map, which its clauses name.
const MEMBER: &str = "requestItems";

/// One write request a row sends: a put of an item keyed `pk`, or a delete of such a key.
#[derive(Clone)]
enum Write {
    Put(String),
    Delete(String),
}

impl Write {
    /// The JSON the request carries this write request as.
    fn json(&self) -> Value {
        match self {
            Self::Put(key) => json!({"PutRequest": {"Item": {"pk": {"S": key}}}}),
            Self::Delete(key) => json!({"DeleteRequest": {"Key": {"pk": {"S": key}}}}),
        }
    }

    /// This write request as a clause about the map quotes it: both members of the
    /// `WriteRequest`, the one it carries rendered and the other unset, each request
    /// structure carrying the workload-profile field the service's own request objects have.
    fn rendered(&self) -> String {
        match self {
            Self::Put(key) => format!(
                "WriteRequest(putRequest=PutRequest(item={{pk={}}}, \
                 workloadProfileName=null), deleteRequest=null)",
                frame("s", key)
            ),
            Self::Delete(key) => format!(
                "WriteRequest(putRequest=null, deleteRequest=DeleteRequest(key={{pk={}}}, \
                 workloadProfileName=null))",
                frame("s", key)
            ),
        }
    }
}

/// The attribute-value frame a clause renders a value in: every field of the service's own
/// attribute value object, in its order, `field` carrying `value` and every other unset.
/// Written out here rather than read off the crate under test, so the RENDERING is what
/// these rows are held to.
fn frame(field: &str, value: &str) -> String {
    const FIELDS: [&str; 16] = [
        "s", "n", "b", "iNT", "dECIMAL", "dOUBLE", "sS", "nS", "bS", "iNTSET", "dECIMALSET",
        "dOUBLESET", "m", "l", "nULL", "bOOL",
    ];
    let rendered: Vec<String> = FIELDS
        .iter()
        .map(|declared| {
            if *declared == field {
                format!("{declared}={value}")
            } else {
                format!("{declared}=null")
            }
        })
        .collect();
    format!("AttributeValue({})", rendered.join(", "))
}

/// The map one row sends: the tables it addresses and the write requests each was given.
type Entries<'a> = Vec<(&'a str, Vec<Write>)>;

/// `count` puts of distinct items, keyed from `prefix`.
fn puts(prefix: &str, count: usize) -> Vec<Write> {
    (0..count)
        .map(|at| Write::Put(format!("{prefix}{at}")))
        .collect()
}

/// `count` deletes of distinct keys, keyed from `prefix`.
fn deletes(prefix: &str, count: usize) -> Vec<Write> {
    (0..count)
        .map(|at| Write::Delete(format!("{prefix}{at}")))
        .collect()
}

/// The `RequestItems` map a row sends, as the JSON the request carries.
fn request_items(entries: &[(&str, Vec<Write>)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(table, writes)| {
                (
                    (*table).to_string(),
                    Value::Array(writes.iter().map(Write::json).collect()),
                )
            })
            .collect(),
    )
}

/// The same map as a clause quotes it: the entries in NAME order, each table's own requests
/// in the order the request listed them.
fn rendered_request_items(entries: &[(&str, Vec<Write>)]) -> String {
    let mut ordered: Vec<&(&str, Vec<Write>)> = entries.iter().collect();
    ordered.sort_by_key(|(table, _)| *table);
    let rendered: Vec<String> = ordered
        .into_iter()
        .map(|(table, writes)| {
            let requests: Vec<String> = writes.iter().map(Write::rendered).collect();
            format!("{table}=[{}]", requests.join(", "))
        })
        .collect();
    format!("{{{}}}", rendered.join(", "))
}

/// The WHOLE message a map whose values broke their bound earns.
fn map_value_refusal(entries: &[(&str, Vec<Write>)]) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{MEMBER}' failed to satisfy constraint: \
         Map value must satisfy constraint: [Member must have length less than or equal to \
         {MOST}, Member must have length greater than or equal to {LEAST}]",
        rendered_request_items(entries)
    )
}

#[tokio::test]
async fn a_map_value_outside_its_declared_bound_is_stated_at_the_map_member() {
    let router = emulator();
    create_active_table(&router, FIRST, &[("pk", "HASH", "S")]).await;
    create_active_table(&router, SECOND, &[("pk", "HASH", "S")]).await;

    // Each row: the case and the map the request sends. The whole message every row must
    // earn is the same rule over the map it sent.
    let rows: Vec<(&str, Entries<'_>)> = vec![
        (
            "one table given one put past the maximum",
            vec![(FIRST, puts("over-", MOST + 1))],
        ),
        (
            "one table given one delete past the maximum",
            vec![(FIRST, deletes("gone-", MOST + 1))],
        ),
        (
            "one table given puts and deletes together, one past the maximum",
            vec![(FIRST, {
                let half = MOST.div_ceil(2);
                let mut writes = puts("mixed-", half);
                writes.extend(deletes("mixed-gone-", MOST + 1 - half));
                writes
            })],
        ),
        (
            "one table given half again past the maximum",
            vec![(FIRST, puts("far-", MOST + MOST / 2))],
        ),
        (
            "one table given no request at all, below the minimum",
            vec![(FIRST, Vec::new())],
        ),
        (
            "two tables, the second given no request",
            vec![(FIRST, puts("kept-", 1)), (SECOND, Vec::new())],
        ),
        (
            // The entry past the maximum is refused beside an entry inside the bound, and
            // the two together also sum past what one call may carry: the MEMBER's bound is
            // stated, not the call's quota.
            "two tables, the first past the maximum and the second inside the bound",
            vec![
                (FIRST, puts("both-", MOST + 1)),
                (SECOND, deletes("both-gone-", 1)),
            ],
        ),
        (
            // The entries written in the order the request did NOT list them: the clause
            // states the map in name order, so one submitted map renders one way.
            "the entries sent in reverse name order",
            vec![(SECOND, puts("rev-", 1)), (FIRST, Vec::new())],
        ),
    ];

    for (case, entries) in rows {
        let (status, body) = call(
            &router,
            "BatchWriteItem",
            json!({"RequestItems": request_items(&entries)}),
        )
        .await;
        let (code, stated) = (error_code(&body), message(&body));
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code} with \
             {stated:?}"
        );
        let expected = map_value_refusal(&entries);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn a_map_whose_values_are_inside_the_bound_is_not_refused_at_that_member() {
    let router = emulator();
    create_active_table(&router, FIRST, &[("pk", "HASH", "S")]).await;
    create_active_table(&router, SECOND, &[("pk", "HASH", "S")]).await;

    // AT each end of the bound, and spread over two entries: served in full, so the rows
    // above are about the bound and not about the operation.
    for (case, entries) in [
        ("one table at the bound's minimum", vec![(FIRST, puts("at-least-", LEAST))]),
        ("one table at the bound's maximum", vec![(FIRST, puts("at-most-", MOST))]),
        (
            "the maximum shared by two tables, each inside the bound",
            vec![
                (FIRST, puts("shared-a-", MOST - 1)),
                (SECOND, puts("shared-b-", 1)),
            ],
        ),
    ] {
        let served = ok(
            &router,
            "BatchWriteItem",
            json!({"RequestItems": request_items(&entries)}),
        )
        .await;
        assert_eq!(
            served["UnprocessedItems"],
            json!({}),
            "case {case:?}: expected every write applied, got {served}"
        );
    }

    // Every entry inside the bound and the OPERATIONS summed past what one call may carry:
    // the call's own quota, stated as the fixed sentence that names no member and echoes
    // nothing of the request.
    let (status, body) = call(
        &router,
        "BatchWriteItem",
        json!({"RequestItems": request_items(&[
            (FIRST, puts("quota-a-", MOST)),
            (SECOND, puts("quota-b-", LEAST)),
        ])}),
    )
    .await;
    let (code, stated) = (error_code(&body), message(&body));
    assert_eq!(
        (status, code.as_str(), stated.as_str()),
        (
            400,
            "ValidationException",
            "Too many items requested for the BatchWriteItem call"
        ),
        "case the call's own quota: expected the fixed aggregate sentence, got {status} {code} \
         with {stated:?}"
    );
}
