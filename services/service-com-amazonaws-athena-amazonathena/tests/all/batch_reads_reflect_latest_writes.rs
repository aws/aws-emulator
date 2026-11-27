// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a BATCHED read of this service reports after the records it
//! addresses have been written, driven over the wire (awsJson1_1,
//! `x-amz-target: AmazonAthena.<Operation>`) through the same router the emulator boots.
//!
//! THE RULE UNDER TEST: a batched read serves the same records the single read of one serves,
//! so each entry it answers with is the addressed record as the LAST write to it left it, and
//! the entries follow the order the request addressed them in rather than any order the store
//! holds the records in.
//!
//! The surface is the model's, read with
//! `smithy_query(query="operation",operation="BatchGetNamedQuery,UpdateNamedQuery,BatchGetPreparedStatement,UpdatePreparedStatement")`:
//! `BatchGetNamedQuery` takes `NamedQueryIds` (required, 1 to 50) and answers with
//! `NamedQueries` and `UnprocessedNamedQueryIds`; `UpdateNamedQuery` carries `NamedQueryId`,
//! `Name` and `QueryString` with an optional `Description` and an empty output;
//! `BatchGetPreparedStatement` takes `WorkGroup` and `PreparedStatementNames` and answers with
//! `PreparedStatements` and `UnprocessedPreparedStatementNames`; `UpdatePreparedStatement`
//! carries `WorkGroup`, `StatementName` and `QueryStatement` with an optional `Description`.
//!
//! Each test asserts the rule over a TABLE rather than one request: the records differ along
//! which members a write changed, whether a description was added, replaced or left standing,
//! how many writes a record took and which workgroup holds it, and the batch rows differ along
//! how many addresses they carry, in which order, whether an address repeats and whether one
//! of them resolves to nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_athena_amazonathena::state::{
    named_query_not_found, prepared_statement_name_not_found, DEFAULT_WORK_GROUP,
    ERROR_CODE_NAMED_QUERY_NOT_FOUND, ERROR_CODE_STATEMENT_NAME_NOT_FOUND,
};

use crate::harness::{call, create_work_group, router};

/// A workgroup of its own, so a row covers a record outside the account's default workgroup
/// as well as inside it.
const NAMED_QUERY_WORK_GROUP: &str = "batch-freshness-named-queries";

/// The workgroup every prepared statement below belongs to: a prepared statement is addressed
/// within one workgroup.
const PREPARED_STATEMENT_WORK_GROUP: &str = "batch-freshness-prepared-statements";

/// A query text of `statements` statements, so a row can differ along how much text a write
/// carries without quoting any one recorded query.
fn query_text(statements: usize) -> String {
    (0..statements)
        .map(|position| format!("SELECT {position} AS position FROM events"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// A `NamedQueryId` no create ever minted, satisfying the element constraints of
/// `NamedQueryIdList` (1 to 128 characters, `^\S+$`) so the only thing wrong with it is that
/// the registry holds nothing under it.
fn unheld_named_query_id(seed: usize) -> String {
    format!("00000000-0000-4000-8000-{seed:012}")
}

/// A prepared statement name this account holds none under, satisfying the modelled
/// `StatementName` pattern (`^[a-zA-Z_][a-zA-Z0-9_@:]{1,256}$`).
fn unheld_statement_name(seed: usize) -> String {
    format!("unheld_statement_{seed}")
}

/// One record the batch rows address: how it got to its current shape, the address it answers
/// under, and the members a read of it must report.
struct Record {
    /// The row of the write table this record came from, named in every failure.
    case: &'static str,
    /// The `NamedQueryId` the create minted, or the `StatementName` the create carried.
    address: String,
    /// The members a read must report, as the LAST write to the record left them.
    expected: Value,
}

/// Save one named query and answer with the `NamedQueryId` the create minted.
async fn create_named_query(router: &MultiServiceRouter, request: &Value) -> String {
    let (status, body) = call(router, "CreateNamedQuery", request).await;
    assert_eq!(
        status, 200,
        "CreateNamedQuery({request}) must be accepted, got {status} with {body:?}"
    );
    body["NamedQueryId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateNamedQuery must answer with an id, got {body:?}"))
        .to_string()
}

/// `GetNamedQuery`'s view of one saved query — the single read a batch read must agree with.
async fn read_named_query(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, body) = call(router, "GetNamedQuery", &json!({ "NamedQueryId": id })).await;
    assert_eq!(
        status, 200,
        "GetNamedQuery({id}) must answer 200, got {status} with {body:?}"
    );
    body["NamedQuery"].clone()
}

/// The members of a `NamedQuery` a read reports, as a document that can be compared whole.
fn named_query_members(found: &Value) -> Value {
    json!({
        "Name": found["Name"],
        "Database": found["Database"],
        "QueryString": found["QueryString"],
        "Description": found["Description"],
        "WorkGroup": found["WorkGroup"],
    })
}

/// Save the named queries the batch rows address and write the ones whose row carries updates,
/// answering with each record as its last write left it.
///
/// The rows differ along which members the write changed, whether the description was added,
/// replaced or left standing, how many writes the record took, and whether it lives in the
/// account's default workgroup or in one of its own.
async fn given_written_named_queries(router: &MultiServiceRouter) -> Vec<Record> {
    create_work_group(router, NAMED_QUERY_WORK_GROUP).await;

    // (the case, the create body minus the name, the updates applied in order minus the id)
    let rows: Vec<(&'static str, Value, Vec<Value>)> = vec![
        (
            "a query no update ever touched",
            json!({
                "Name": "untouched",
                "Database": "logs",
                "QueryString": query_text(1),
                "Description": "as created",
            }),
            Vec::new(),
        ),
        (
            "a query whose name and text an update replaced",
            json!({
                "Name": "before-rename",
                "Database": "logs",
                "QueryString": query_text(1),
                "Description": "as created",
            }),
            vec![json!({
                "Name": "after-rename",
                "QueryString": query_text(6),
                "Description": "as created",
            })],
        ),
        (
            "a query whose description an update left standing",
            json!({
                "Name": "described-on-create",
                "Database": "logs",
                "QueryString": query_text(2),
                "Description": "as created",
            }),
            vec![json!({
                "Name": "undescribed-now",
                "QueryString": query_text(3),
            })],
        ),
        (
            "a query a description was added to",
            json!({
                "Name": "undescribed-on-create",
                "Database": "analytics",
                "QueryString": query_text(1),
            }),
            vec![json!({
                "Name": "described-now",
                "QueryString": query_text(2),
                "Description": "added by the update",
            })],
        ),
        (
            "a query two updates wrote, so the last write stands",
            json!({
                "Name": "first-name",
                "Database": "analytics",
                "QueryString": query_text(1),
            }),
            vec![
                json!({
                    "Name": "second-name",
                    "QueryString": query_text(2),
                    "Description": "written by the first update",
                }),
                json!({
                    "Name": "third-name",
                    "QueryString": query_text(9),
                    "Description": "written by the second update",
                }),
            ],
        ),
        (
            "a query of its own workgroup an update wrote",
            json!({
                "Name": "scoped-before",
                "Database": "warehouse",
                "QueryString": query_text(2),
                "WorkGroup": NAMED_QUERY_WORK_GROUP,
            }),
            vec![json!({
                "Name": "scoped-after",
                // A description at the modelled maximum (1024 characters), so a row differs
                // along the SIZE of what the write carries as well.
                "Description": "d".repeat(1024),
                "QueryString": query_text(40),
            })],
        ),
    ];

    let mut records = Vec::new();
    for (case, create, updates) in rows {
        let address = create_named_query(router, &create).await;
        let mut expected = json!({
            "Name": create["Name"],
            "Database": create["Database"],
            "QueryString": create["QueryString"],
            "Description": create.get("Description").cloned().unwrap_or(Value::Null),
            // Neither member an update may carry: the workgroup the create named, or the
            // account's default workgroup when it named none.
            "WorkGroup": json!(create
                .get("WorkGroup")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_WORK_GROUP)),
        });
        for update in &updates {
            let mut request = update.clone();
            request["NamedQueryId"] = json!(address);
            let (status, body) = call(router, "UpdateNamedQuery", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateNamedQuery({request}) must be accepted, got {status} with \
                 {body:?}"
            );
            expected["Name"] = update["Name"].clone();
            expected["QueryString"] = update["QueryString"].clone();
            // An update that carries no description leaves the one that stands, so the
            // expectation only moves when the request named the member.
            if let Some(description) = update.get("Description") {
                expected["Description"] = description.clone();
            }
        }
        records.push(Record {
            case,
            address,
            expected,
        });
    }
    records
}

/// One batch row: which addresses it sends, in which order.
struct BatchRow {
    case: &'static str,
    addresses: Vec<String>,
}

/// The orders a batch of named query ids is sent in: the rows differ along how many ids they
/// carry, their order relative to the order the creates ran in and the order the ids sort in,
/// whether an id repeats, and whether one of them resolves to nothing.
fn named_query_batch_rows(records: &[Record]) -> Vec<BatchRow> {
    let created: Vec<String> = records
        .iter()
        .map(|record| record.address.clone())
        .collect();
    let mut reversed = created.clone();
    reversed.reverse();
    let mut ascending = created.clone();
    ascending.sort();
    let mut descending = ascending.clone();
    descending.reverse();

    vec![
        BatchRow {
            case: "one written id alone",
            addresses: vec![created[1].clone()],
        },
        BatchRow {
            case: "every id in the order the creates ran",
            addresses: created.clone(),
        },
        BatchRow {
            case: "every id in the reverse of that order",
            addresses: reversed,
        },
        BatchRow {
            case: "every id in ascending id order",
            addresses: ascending,
        },
        BatchRow {
            case: "every id in descending id order",
            addresses: descending,
        },
        BatchRow {
            case: "the written ids before the untouched one",
            addresses: vec![
                created[4].clone(),
                created[1].clone(),
                created[5].clone(),
                created[0].clone(),
            ],
        },
        BatchRow {
            case: "a written id repeated around an untouched one",
            addresses: vec![created[2].clone(), created[0].clone(), created[2].clone()],
        },
        BatchRow {
            case: "an id the registry holds nothing under between two written ones",
            addresses: vec![
                created[3].clone(),
                unheld_named_query_id(1),
                created[1].clone(),
            ],
        },
        BatchRow {
            case: "nothing but ids the registry holds nothing under",
            addresses: vec![unheld_named_query_id(2), unheld_named_query_id(3)],
        },
    ]
}

/// A batch read of named queries reports every addressed query as the last write to it left it,
/// in the order the request listed the ids.
#[tokio::test]
async fn batch_get_named_query_answers_the_last_write_in_request_order() {
    let router = router();
    let records = given_written_named_queries(&router).await;

    for BatchRow { case, addresses } in named_query_batch_rows(&records) {
        let (status, body) = call(
            &router,
            "BatchGetNamedQuery",
            &json!({ "NamedQueryIds": addresses }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the batch to be served, got {status} with {body:?}"
        );
        let answered = body["NamedQueries"].as_array().cloned().unwrap_or_default();
        let unprocessed = body["UnprocessedNamedQueryIds"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        // What the rule requires of this row: one entry per id the registry holds a query
        // under, in the order the request listed them, and one unprocessed report per id it
        // holds none under, likewise in order.
        let held: Vec<&Record> = addresses
            .iter()
            .filter_map(|address| records.iter().find(|record| record.address == *address))
            .collect();
        let unheld: Vec<String> = addresses
            .iter()
            .filter(|address| !records.iter().any(|record| record.address == **address))
            .cloned()
            .collect();

        assert_eq!(
            answered.len(),
            held.len(),
            "case {case:?}: expected one entry per id the registry holds, got {answered:?}"
        );
        for (position, record) in held.iter().enumerate() {
            let entry = &answered[position];
            assert_eq!(
                entry["NamedQueryId"].as_str(),
                Some(record.address.as_str()),
                "case {case:?}: expected entry {position} to be the id at that position of the \
                 request, got {entry:?}"
            );
            assert_eq!(
                named_query_members(entry),
                record.expected,
                "case {case:?}: expected entry {position} ({}) to carry the last write, got \
                 {entry:?}",
                record.case
            );
            assert_eq!(
                entry,
                &read_named_query(&router, &record.address).await,
                "case {case:?}: expected entry {position} ({}) to be what a single read of it \
                 answers, got {entry:?}",
                record.case
            );
        }

        let missed: Vec<&str> = unprocessed
            .iter()
            .map(|entry| entry["NamedQueryId"].as_str().unwrap_or("<no NamedQueryId>"))
            .collect();
        let expected_missed: Vec<&str> = unheld.iter().map(String::as_str).collect();
        assert_eq!(
            missed, expected_missed,
            "case {case:?}: expected the unheld ids reported in request order, got \
             {unprocessed:?}"
        );
        for entry in &unprocessed {
            let id = entry["NamedQueryId"].as_str().unwrap_or_default();
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(ERROR_CODE_NAMED_QUERY_NOT_FOUND),
                "case {case:?}: expected the class of a saved query that does not exist, got \
                 {entry:?}"
            );
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some(named_query_not_found(id).as_str()),
                "case {case:?}: expected the sentence a single read of that id states, got \
                 {entry:?}"
            );
        }
    }

    // Reading changed nothing: every record still reads back as its last WRITE left it.
    for record in &records {
        let found = read_named_query(&router, &record.address).await;
        assert_eq!(
            named_query_members(&found),
            record.expected,
            "case {:?}: the batch reads must have written nothing, got {found:?}",
            record.case
        );
    }
}

/// Save one prepared statement in [`PREPARED_STATEMENT_WORK_GROUP`].
async fn create_prepared_statement(router: &MultiServiceRouter, request: &Value) {
    let (status, body) = call(router, "CreatePreparedStatement", request).await;
    assert_eq!(
        status, 200,
        "CreatePreparedStatement({request}) must be accepted, got {status} with {body:?}"
    );
}

/// The members of a `PreparedStatement` whose values a write decides, as a comparable document.
///
/// `LastModifiedTime` is left out on purpose: it is the instant of the write, which is a value
/// the service generates per call rather than one the rule under test is about.
fn prepared_statement_members(found: &Value) -> Value {
    json!({
        "StatementName": found["StatementName"],
        "QueryStatement": found["QueryStatement"],
        "Description": found["Description"],
        "WorkGroupName": found["WorkGroupName"],
    })
}

/// Save the prepared statements the batch rows address and write the ones whose row carries
/// updates, answering with each record as its last write left it.
///
/// The names are deliberately not in alphabetical order of creation, so a row sending them in
/// alphabetical order addresses them in an order no write ran in.
async fn given_written_prepared_statements(router: &MultiServiceRouter) -> Vec<Record> {
    create_work_group(router, PREPARED_STATEMENT_WORK_GROUP).await;

    // (the case, the name, the description the create carried, the updates applied in order as
    //  (query statement, description))
    type Update = (String, Option<&'static str>);
    let rows: Vec<(&'static str, &'static str, Option<&'static str>, Vec<Update>)> = vec![
        (
            "a statement no update ever touched",
            "zeta_statement",
            Some("as created"),
            Vec::new(),
        ),
        (
            "a statement whose text and description an update replaced",
            "mid_statement",
            Some("as created"),
            vec![(query_text(4), Some("written by the update"))],
        ),
        (
            "a statement whose description an update left standing",
            "alpha_statement",
            Some("as created"),
            vec![(query_text(2), None)],
        ),
        (
            "a statement two updates wrote, so the last write stands",
            "omega_statement",
            None,
            vec![
                (query_text(2), Some("written by the first update")),
                (query_text(7), Some("written by the second update")),
            ],
        ),
    ];

    let mut records = Vec::new();
    for (case, name, created_description, updates) in rows {
        let mut create = json!({
            "WorkGroup": PREPARED_STATEMENT_WORK_GROUP,
            "StatementName": name,
            "QueryStatement": query_text(1),
        });
        if let Some(description) = created_description {
            create["Description"] = json!(description);
        }
        create_prepared_statement(router, &create).await;
        let mut expected = json!({
            "StatementName": name,
            "QueryStatement": query_text(1),
            "Description": created_description.map_or(Value::Null, |text| json!(text)),
            "WorkGroupName": PREPARED_STATEMENT_WORK_GROUP,
        });
        for (query_statement, description) in &updates {
            let mut request = json!({
                "WorkGroup": PREPARED_STATEMENT_WORK_GROUP,
                "StatementName": name,
                "QueryStatement": query_statement,
            });
            if let Some(description) = description {
                request["Description"] = json!(description);
            }
            let (status, body) = call(router, "UpdatePreparedStatement", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdatePreparedStatement({request}) must be accepted, got {status} \
                 with {body:?}"
            );
            expected["QueryStatement"] = json!(query_statement);
            // An update that carries no description leaves the one that stands, so the
            // expectation only moves when the request named the member.
            if let Some(description) = description {
                expected["Description"] = json!(description);
            }
        }
        records.push(Record {
            case,
            address: name.to_string(),
            expected,
        });
    }
    records
}

/// `GetPreparedStatement`'s view of one statement — the single read a batch read must agree
/// with.
async fn read_prepared_statement(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(
        router,
        "GetPreparedStatement",
        &json!({ "WorkGroup": PREPARED_STATEMENT_WORK_GROUP, "StatementName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetPreparedStatement({name}) must answer 200, got {status} with {body:?}"
    );
    body["PreparedStatement"].clone()
}

/// One prepared statement batch row: the names it sends, the names it must answer with as
/// statements and the names it must report as unprocessed, each in the order the rule requires.
struct StatementBatchRow {
    case: &'static str,
    sent: Vec<String>,
    answered: Vec<String>,
    unprocessed: Vec<String>,
}

/// The same rule over the service's other batched read: a batch of prepared statement names
/// reports each statement as the last write to it left it, in the order the request named them.
#[tokio::test]
async fn batch_get_prepared_statement_answers_the_last_write_in_request_order() {
    let router = router();
    let records = given_written_prepared_statements(&router).await;
    let created: Vec<String> = records
        .iter()
        .map(|record| record.address.clone())
        .collect();
    let mut reversed = created.clone();
    reversed.reverse();
    let mut alphabetical = created.clone();
    alphabetical.sort();

    // A name sent twice names ONE statement, so it is answered once, at the position of its
    // first mention.
    let rows: Vec<StatementBatchRow> = vec![
        StatementBatchRow {
            case: "one written name alone",
            sent: vec![created[1].clone()],
            answered: vec![created[1].clone()],
            unprocessed: Vec::new(),
        },
        StatementBatchRow {
            case: "every name in the order the creates ran",
            sent: created.clone(),
            answered: created.clone(),
            unprocessed: Vec::new(),
        },
        StatementBatchRow {
            case: "every name in the reverse of that order",
            sent: reversed.clone(),
            answered: reversed,
            unprocessed: Vec::new(),
        },
        StatementBatchRow {
            case: "every name in alphabetical order",
            sent: alphabetical.clone(),
            answered: alphabetical,
            unprocessed: Vec::new(),
        },
        StatementBatchRow {
            case: "a written name repeated around an untouched one",
            sent: vec![created[3].clone(), created[0].clone(), created[3].clone()],
            answered: vec![created[3].clone(), created[0].clone()],
            unprocessed: Vec::new(),
        },
        StatementBatchRow {
            case: "a name the workgroup holds nothing under between two written ones",
            sent: vec![
                created[2].clone(),
                unheld_statement_name(1),
                created[1].clone(),
            ],
            answered: vec![created[2].clone(), created[1].clone()],
            unprocessed: vec![unheld_statement_name(1)],
        },
    ];

    for StatementBatchRow {
        case,
        sent,
        answered: expected_answered,
        unprocessed: expected_unprocessed,
    } in rows
    {
        let (status, body) = call(
            &router,
            "BatchGetPreparedStatement",
            &json!({
                "WorkGroup": PREPARED_STATEMENT_WORK_GROUP,
                "PreparedStatementNames": sent,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the batch to be served, got {status} with {body:?}"
        );
        let answered = body["PreparedStatements"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let names: Vec<&str> = answered
            .iter()
            .map(|entry| entry["StatementName"].as_str().unwrap_or("<no StatementName>"))
            .collect();
        let expected_names: Vec<&str> = expected_answered.iter().map(String::as_str).collect();
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected the held names answered in request order, got {answered:?}"
        );
        for (position, name) in expected_answered.iter().enumerate() {
            let record = records
                .iter()
                .find(|record| &record.address == name)
                .unwrap_or_else(|| panic!("case {case:?}: {name} is one of the written records"));
            let entry = &answered[position];
            assert_eq!(
                prepared_statement_members(entry),
                record.expected,
                "case {case:?}: expected entry {position} ({}) to carry the last write, got \
                 {entry:?}",
                record.case
            );
            assert_eq!(
                prepared_statement_members(entry),
                prepared_statement_members(&read_prepared_statement(&router, name).await),
                "case {case:?}: expected entry {position} ({}) to be what a single read of it \
                 answers, got {entry:?}",
                record.case
            );
        }

        let unprocessed = body["UnprocessedPreparedStatementNames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let missed: Vec<&str> = unprocessed
            .iter()
            .map(|entry| entry["StatementName"].as_str().unwrap_or("<no StatementName>"))
            .collect();
        let expected_missed: Vec<&str> = expected_unprocessed.iter().map(String::as_str).collect();
        assert_eq!(
            missed, expected_missed,
            "case {case:?}: expected the unheld names reported in request order, got \
             {unprocessed:?}"
        );
        for entry in &unprocessed {
            let name = entry["StatementName"].as_str().unwrap_or_default();
            assert_eq!(
                entry["ErrorCode"].as_str(),
                Some(ERROR_CODE_STATEMENT_NAME_NOT_FOUND),
                "case {case:?}: expected the class a missing statement is reported under, got \
                 {entry:?}"
            );
            assert_eq!(
                entry["ErrorMessage"].as_str(),
                Some(prepared_statement_name_not_found(name).as_str()),
                "case {case:?}: expected the reason naming the statement asked for, got \
                 {entry:?}"
            );
        }
    }

    // Reading changed nothing: every statement still reads back as its last WRITE left it.
    for record in &records {
        let found = read_prepared_statement(&router, &record.address).await;
        assert_eq!(
            prepared_statement_members(&found),
            record.expected,
            "case {:?}: the batch reads must have written nothing, got {found:?}",
            record.case
        );
    }
}
