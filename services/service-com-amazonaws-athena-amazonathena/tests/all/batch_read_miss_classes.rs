// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CLASS a batch read reports an id it could not resolve under,
//! driven over the wire (awsJson1_1, `x-amz-target: AmazonAthena.<Operation>`) through the
//! same router the emulator boots.
//!
//! THE RULE UNDER TEST: a batch read answers in two parts instead of refusing, and the
//! per-item entry it reports an unresolved id under carries THAT RESOURCE KIND's own
//! not-found class under `ErrorCode` — never the class of a bad request, which Athena
//! reserves for a request the caller must correct and which a batch read uses only when the
//! request itself is refused whole, before any id is resolved.
//!
//! The rule is asserted over every batch read this service serves, each with its own class
//! and its own sentence for a miss:
//!
//! * `BatchGetNamedQuery` → `NAMED_QUERY_NOT_FOUND`;
//! * `BatchGetQueryExecution` → `QUERY_EXECUTION_NOT_FOUND`;
//! * `BatchGetPreparedStatement` → `STATEMENTNAME_NOT_FOUND`, the code Athena answers on
//!   the wire for a name the workgroup holds nothing under, beside the `INVALID_INPUT` of a
//!   name that "is not valid" — the two classes the rule separates.
//!
//! The rows of each table differ along the FORM and the SIZE of the id that resolves to
//! nothing, inside the constraints the model declares on it (`NamedQueryId` and
//! `QueryExecutionId`: 1 to 128 characters matching `^\S+$`; `StatementName`: 1 to 256
//! characters matching `^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$`, read with
//! `smithy_query(query="shape",shape="NamedQueryId,QueryExecutionId,StatementName")`), and
//! one row per kind is a HELD id with one character changed, so no row is a written-down
//! identifier. The second test covers the other half of the rule: a list element OUTSIDE
//! those constraints makes the request itself wrong, so it is refused whole with the class
//! of a bad request and nothing is partitioned.

use serde_json::{json, Map, Value};
use service_com_amazonaws_athena_amazonathena::state::{
    self, ERROR_CODE_INVALID_INPUT, ERROR_CODE_NAMED_QUERY_NOT_FOUND,
    ERROR_CODE_QUERY_EXECUTION_NOT_FOUND, ERROR_CODE_STATEMENT_NAME_NOT_FOUND,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_work_group, error_code, message, router, start};

/// Where the query this suite starts writes its results: what matters is that the query HAS
/// a location, so the start is accepted and really mints an id.
const OUTPUT_LOCATION: &str = "s3://athena-results/batch-read-miss-classes/";

/// The workgroup the prepared statement of this suite belongs to: a prepared statement is
/// addressed within one workgroup.
const WORK_GROUP: &str = "batch-read-miss-classes";

/// The error shape a refused request carries: the client error every batch read models
/// (HTTP 400).
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The widest an `ErrorCode` may be: the modelled `ErrorCode` `@length` (1 to 256).
const MAX_ERROR_CODE_CHARS: usize = 256;

/// One batch read, as this suite drives it: the operation, the members it answers in, the
/// class a miss of one of its ids carries, and what this account holds for it.
struct BatchRead {
    /// What the batch read resolves, for the case name an assertion reports.
    kind: &'static str,
    /// The operation's name, as `x-amz-target` carries it.
    operation: &'static str,
    /// The input member listing the ids to resolve.
    ids_member: &'static str,
    /// The output member the resolved resources come back in.
    found_member: &'static str,
    /// The output member an unresolved id is reported in.
    unprocessed_member: &'static str,
    /// The member that echoes the id, in the resolved half and the unprocessed half alike.
    id_member: &'static str,
    /// Any other required input member (the workgroup a prepared statement is addressed in).
    extra: Value,
    /// The class a well-formed id this account resolves nothing under is reported under.
    class: &'static str,
    /// The sentence that miss is stated in.
    miss_message: fn(&str) -> String,
    /// An id this account really holds, asked for beside every miss so the answer is a
    /// PARTITION rather than one class for everything.
    held: String,
    /// The ids that resolve to nothing: `(case, id)`.
    misses: Vec<(&'static str, String)>,
}

impl BatchRead {
    /// The request that asks this batch read for `ids`.
    fn request(&self, ids: &[String]) -> Value {
        let mut request = Map::new();
        request.insert(self.ids_member.to_string(), json!(ids));
        if let Some(members) = self.extra.as_object() {
            for (member, value) in members {
                request.insert(member.clone(), value.clone());
            }
        }
        Value::Object(request)
    }

    /// Ask for `ids`: `(status, the ids answered, the unprocessed entries)`.
    async fn read(&self, router: &MultiServiceRouter, ids: &[String]) -> (u16, Vec<String>, Vec<Value>) {
        let (status, body) = call(router, self.operation, &self.request(ids)).await;
        let answered = body[self.found_member]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|found| {
                found[self.id_member]
                    .as_str()
                    .unwrap_or("<no id>")
                    .to_string()
            })
            .collect();
        let unprocessed = body[self.unprocessed_member]
            .as_array()
            .cloned()
            .unwrap_or_default();
        (status, answered, unprocessed)
    }
}

/// The sentence a miss of a `NamedQueryId` is stated in.
fn named_query_miss(id: &str) -> String {
    state::named_query_not_found(id)
}

/// The sentence a miss of a `QueryExecutionId` is stated in.
fn query_execution_miss(id: &str) -> String {
    state::query_execution_not_found(id)
}

/// The sentence a miss of a `StatementName` is stated in: Athena's wording for that miss
/// names the statement, so the name the request carried is its one variable part.
fn prepared_statement_miss(name: &str) -> String {
    state::prepared_statement_name_not_found(name)
}

/// `held` with its last character replaced: an id of exactly the form this service holds,
/// which nothing answers to. Built FROM state, so the row is not a written-down identifier,
/// and the replacement stays inside every constraint the ids of this suite satisfy.
fn flip_last(held: &str) -> String {
    let mut near_miss = held.to_string();
    let last = near_miss.pop().expect("a held id is not empty");
    near_miss.push(if last == 'z' { 'y' } else { 'z' });
    near_miss
}

/// The ids that resolve to nothing for a kind addressed by a `NamedQueryId`- or
/// `QueryExecutionId`-shaped id (1 to 128 characters, `^\S+$`).
fn id_misses(held: &str) -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model admits", "x".to_string()),
        ("the longest id the model admits", "x".repeat(128)),
        (
            "a hyphenated UUID nothing was minted under",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        ),
        (
            "the same UUID in upper case",
            "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE".to_string(),
        ),
        (
            "a word that is no UUID at all",
            "no-such-resource".to_string(),
        ),
        ("an id carrying punctuation", "not/a::resource#id".to_string()),
        ("a held id with its last character changed", flip_last(held)),
    ]
}

/// The names that resolve to nothing for a prepared statement (1 to 256 characters,
/// `^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$`).
fn name_misses(held: &str) -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name the pattern admits", "ab".to_string()),
        (
            "the longest name the model admits",
            format!("a{}", "b".repeat(255)),
        ),
        (
            "a name carrying the punctuation the pattern admits",
            "unheld_statement@v1:2".to_string(),
        ),
        (
            "a name starting with an underscore",
            "_unheld_statement".to_string(),
        ),
        ("a held name with its last character changed", flip_last(held)),
    ]
}

/// Every batch read this service serves, each with one resource this account really holds.
async fn batch_reads(router: &MultiServiceRouter) -> Vec<BatchRead> {
    create_work_group(router, WORK_GROUP).await;

    let (status, saved) = call(
        router,
        "CreateNamedQuery",
        &json!({
            "Name": "held",
            "Database": "logs",
            "QueryString": "SELECT count(*) FROM requests",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the named query must be created, got {status} with {saved:?}"
    );
    let named_query_id = saved["NamedQueryId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNamedQuery must mint an id, got {saved:?}"))
        .to_string();

    let execution_id = start(
        router,
        &json!({
            "QueryString": "SELECT 1 AS n",
            "ResultConfiguration": { "OutputLocation": OUTPUT_LOCATION },
        }),
    )
    .await;

    let statement_name = "held_statement".to_string();
    let (status, created) = call(
        router,
        "CreatePreparedStatement",
        &json!({
            "StatementName": statement_name,
            "WorkGroup": WORK_GROUP,
            "QueryStatement": "SELECT * FROM requests WHERE status = ?",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the prepared statement must be created, got {status} with {created:?}"
    );

    vec![
        BatchRead {
            kind: "a saved query",
            operation: "BatchGetNamedQuery",
            ids_member: "NamedQueryIds",
            found_member: "NamedQueries",
            unprocessed_member: "UnprocessedNamedQueryIds",
            id_member: "NamedQueryId",
            extra: json!({}),
            class: ERROR_CODE_NAMED_QUERY_NOT_FOUND,
            miss_message: named_query_miss,
            misses: id_misses(&named_query_id),
            held: named_query_id,
        },
        BatchRead {
            kind: "a query execution",
            operation: "BatchGetQueryExecution",
            ids_member: "QueryExecutionIds",
            found_member: "QueryExecutions",
            unprocessed_member: "UnprocessedQueryExecutionIds",
            id_member: "QueryExecutionId",
            extra: json!({}),
            class: ERROR_CODE_QUERY_EXECUTION_NOT_FOUND,
            miss_message: query_execution_miss,
            misses: id_misses(&execution_id),
            held: execution_id,
        },
        BatchRead {
            kind: "a prepared statement",
            operation: "BatchGetPreparedStatement",
            ids_member: "PreparedStatementNames",
            found_member: "PreparedStatements",
            unprocessed_member: "UnprocessedPreparedStatementNames",
            id_member: "StatementName",
            extra: json!({ "WorkGroup": WORK_GROUP }),
            class: ERROR_CODE_STATEMENT_NAME_NOT_FOUND,
            miss_message: prepared_statement_miss,
            misses: name_misses(&statement_name),
            held: statement_name,
        },
    ]
}

/// A well-formed id a batch read resolves nothing under is reported IN BAND, under the
/// not-found class of the resource kind that batch read resolves — not under the class of a
/// bad request — beside the sentence that miss is stated in, while a resource asked for in
/// the same request still comes back.
#[tokio::test]
async fn an_unresolvable_id_is_reported_under_its_resource_kinds_own_not_found_class() {
    let router = router();
    let reads = batch_reads(&router).await;

    for read in &reads {
        for (case, id) in &read.misses {
            let case = format!("{} / {case}", read.kind);
            let (status, answered, unprocessed) =
                read.read(&router, &[read.held.clone(), id.clone()]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: an unresolvable id is reported, not refused, got {status}"
            );
            assert_eq!(
                answered,
                vec![read.held.clone()],
                "case {case:?}: expected the held id alone under {}, got {answered:?}",
                read.found_member
            );
            assert_eq!(
                unprocessed.len(),
                1,
                "case {case:?}: expected exactly one unprocessed entry, got {unprocessed:?}"
            );
            let entry = &unprocessed[0];
            assert_eq!(
                entry[read.id_member].as_str(),
                Some(id.as_str()),
                "case {case:?}: expected the id it was asked for, got {entry:?}"
            );
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(read.class),
                "case {case:?}: expected the class {:?}, got {entry:?}",
                read.class
            );
            assert_ne!(
                entry["ErrorCode"].as_str(),
                Some(ERROR_CODE_INVALID_INPUT),
                "case {case:?}: a well-formed id that resolves to nothing is no bad request, \
                 got {entry:?}"
            );
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some((read.miss_message)(id).as_str()),
                "case {case:?}: expected the sentence that miss is stated in, got {entry:?}"
            );
            let code = entry["ErrorCode"].as_str().unwrap_or_default();
            assert!(
                (1..=MAX_ERROR_CODE_CHARS).contains(&code.chars().count()),
                "case {case:?}: the class must satisfy the modelled ErrorCode length, got \
                 {entry:?}"
            );
        }
    }

    // Every class is a class of its OWN: a client branching on the per-item code can tell
    // which kind of resource was missing, and none of them is the bad-request class.
    let classes: Vec<&str> = reads.iter().map(|read| read.class).collect();
    for (position, class) in classes.iter().enumerate() {
        assert!(
            !classes[position + 1..].contains(class),
            "every batch read must report its own class, got {classes:?}"
        );
    }

    // Reading changed nothing: every held resource is still answered, so the misses above
    // cost no caller the resource it holds.
    for read in &reads {
        let (status, answered, unprocessed) =
            read.read(&router, std::slice::from_ref(&read.held)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the held id must still be answered, got {status}",
            read.kind
        );
        assert_eq!(
            answered,
            vec![read.held.clone()],
            "case {:?}: expected the held id answered, got {answered:?} with {unprocessed:?}",
            read.kind
        );
    }
}

/// A list element OUTSIDE the modelled identifier is the request itself being wrong: the
/// whole batch is refused with the class a bad request carries, under both names the wire
/// gives a refusal's code, and nothing is partitioned.
#[tokio::test]
async fn an_id_outside_the_modelled_identifier_refuses_the_whole_batch_as_invalid_input() {
    let router = router();
    let reads = batch_reads(&router).await;

    // The rows differ along the constraint the id breaks: each published bound of the
    // identifier and one step past it, and its pattern.
    let rows: Vec<(&str, String)> = vec![
        (
            "no characters at all, one step below the minimum",
            String::new(),
        ),
        ("one character past the longest id", "x".repeat(129)),
        ("an id carrying a space", "has a space".to_string()),
        ("an id that is whitespace alone", " ".to_string()),
        ("an id carrying a tab", "has\ta-tab".to_string()),
    ];

    // The identifiers of the two id-shaped batch reads share those constraints; a prepared
    // statement name has a pattern of its own, so it is refused for breaking that one.
    for read in reads
        .iter()
        .filter(|read| read.id_member != "StatementName")
    {
        for (case, id) in &rows {
            let case = format!("{} / {case}", read.kind);
            let (status, body) = call(
                &router,
                read.operation,
                &read.request(&[read.held.clone(), id.clone()]),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_REQUEST,
                "case {case:?}: expected {INVALID_REQUEST}, got {body:?}"
            );
            for member in ["AthenaErrorCode", "ErrorCode"] {
                assert_eq!(
                    body[member].as_str(),
                    Some(ERROR_CODE_INVALID_INPUT),
                    "case {case:?}: expected {member} {ERROR_CODE_INVALID_INPUT:?}, got {body:?}"
                );
            }
            assert!(
                body[read.found_member].is_null() && body[read.unprocessed_member].is_null(),
                "case {case:?}: a refused request partitions nothing, got {body:?}"
            );
            assert!(
                !message(&body).is_empty(),
                "case {case:?}: a refusal states what the request broke, got {body:?}"
            );
        }
    }

    // A name breaking the prepared statement pattern is refused the same way: one bad
    // element, one refused request, nothing partitioned.
    let statements = reads
        .iter()
        .find(|read| read.id_member == "StatementName")
        .expect("the prepared statement batch read is in the table");
    for (case, name) in [
        ("a name starting with a digit", "0leading-digit".to_string()),
        ("a name carrying a space", "has a space".to_string()),
        (
            "one character past the longest name",
            format!("a{}", "b".repeat(256)),
        ),
    ] {
        let (status, body) = call(
            &router,
            statements.operation,
            &statements.request(&[statements.held.clone(), name.clone()]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        for member in ["AthenaErrorCode", "ErrorCode"] {
            assert_eq!(
                body[member].as_str(),
                Some(ERROR_CODE_INVALID_INPUT),
                "case {case:?}: expected {member} {ERROR_CODE_INVALID_INPUT:?}, got {body:?}"
            );
        }
        assert!(
            body[statements.found_member].is_null()
                && body[statements.unprocessed_member].is_null(),
            "case {case:?}: a refused request partitions nothing, got {body:?}"
        );
    }
}
