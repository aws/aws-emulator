// SPDX-License-Identifier: CC0-1.0

//! What the Data API records and reports for a SUBMITTED statement, driven over
//! the wire: every call below is a real `POST /` with
//! `X-Amz-Target: RedshiftData.<Operation>` dispatched through the router.
//!
//! Four rules, each asserted over a TABLE of inputs rather than over the one
//! request that showed it:
//!
//! * a statement's result format is REPORTED in the service's own spelling —
//!   the lower-case form of the modelled token — by every member that reports
//!   it, while the submit keeps accepting the modelled upper-case tokens;
//! * a statement's rows are read by the reader that serves the format the
//!   statement was SUBMITTED with, and refused by the other one;
//! * a submission's named `Parameters` are resolved against the placeholders its
//!   SQL really references before it is admitted: a placeholder nothing
//!   supplies, a parameter no statement references and a name supplied twice are
//!   refused, and a refused submission records no statement;
//! * a submit that replays a `ClientToken` runs nothing and mints nothing, but
//!   the statement's record reports the text and the parameters LAST submitted
//!   under that token.

use serde_json::{json, Value};

use super::emulator::{error_code, id_of, list, number, timestamp, Emulator};

/// The connection every statement in this suite runs through.
fn cluster() -> Value {
    json!({"ClusterIdentifier": "my-cluster", "Database": "dev", "DbUser": "awsuser"})
}

/// The `Message` a handler's own refusal carries.
fn message(response: &Value) -> String {
    response["Message"].as_str().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// The spelling a result format is reported in
// ---------------------------------------------------------------------------

/// One row of the result-format table: what the submit asked for, and the
/// spelling every reporting member has to carry.
struct FormatCase {
    label: &'static str,
    /// The `ResultFormat` the submit sends, or `None` for a submit that sends
    /// none at all.
    submitted: Option<&'static str>,
    reported: &'static str,
}

/// A statement's `ResultFormat` is reported in the lower-case spelling of the
/// token it was submitted with, by every member that reports it, while the
/// submit accepts the modelled tokens.
///
/// The tokens the input accepts are `ResultFormatString`'s own values, `CSV` and
/// `JSON` (`smithy_query(query="shape",shape="ResultFormatString")`), plus the
/// absent member, which the operation documents as defaulting to JSON.
#[tokio::test]
async fn a_result_format_is_reported_in_the_services_own_lower_case_spelling() {
    let emulator = Emulator::new();
    let cases = [
        FormatCase {
            label: "submitted as the modelled CSV token",
            submitted: Some("CSV"),
            reported: "csv",
        },
        FormatCase {
            label: "submitted as the modelled JSON token",
            submitted: Some("JSON"),
            reported: "json",
        },
        FormatCase {
            label: "no format submitted at all",
            submitted: None,
            reported: "json",
        },
    ];

    for case in &cases {
        let mut request = cluster();
        request["Sql"] = json!("SELECT 1");
        if let Some(submitted) = case.submitted {
            request["ResultFormat"] = json!(submitted);
        }
        let submitted = emulator.execute_and_wait(&request).await;
        let id = id_of(&submitted);

        let described = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            described["ResultFormat"], case.reported,
            "case {:?}: DescribeStatement must report {:?}, got {described}",
            case.label, case.reported
        );

        let page = emulator.ok("ListStatements", &json!({})).await;
        let listed = list(&page, "Statements")
            .into_iter()
            .find(|statement| statement["Id"] == json!(id))
            .unwrap_or_else(|| panic!("case {:?}: the statement must be listed", case.label));
        assert_eq!(
            listed["ResultFormat"], case.reported,
            "case {:?}: ListStatements must report {:?}, got {listed}",
            case.label, case.reported
        );
        assert_eq!(
            described["Status"], "FINISHED",
            "case {:?}: the submitted token must still be accepted, got {described}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// Which reader serves a statement's rows
// ---------------------------------------------------------------------------

/// One row of the CSV-rendering table: the expression a statement selects, and
/// the cell the CSV form has to carry for it.
struct CellCase {
    label: &'static str,
    /// The expression selected as the first column of a two-column row, so an
    /// empty cell is read as the empty cell it is and not as an empty record.
    selected: &'static str,
    cell: &'static str,
}

/// A statement submitted in the CSV format is read by the CSV reader, which
/// answers one record carrying the result's header line and its rows, with the
/// format it reports in the service's own spelling.
#[tokio::test]
async fn the_csv_reader_answers_a_csv_statements_rows_as_csv_text() {
    let emulator = Emulator::new();
    let cases = [
        CellCase {
            label: "a plain string",
            selected: "'plain'",
            cell: "plain",
        },
        CellCase {
            label: "a string carrying the separator",
            selected: "'has,comma'",
            cell: "\"has,comma\"",
        },
        CellCase {
            label: "a string carrying a quote",
            selected: "'has\"quote'",
            cell: "\"has\"\"quote\"",
        },
        CellCase {
            label: "a string carrying a line break",
            selected: "'a' || char(10) || 'b'",
            cell: "\"a\nb\"",
        },
        CellCase {
            label: "an integer",
            selected: "42",
            cell: "42",
        },
        CellCase {
            label: "an approximate number",
            selected: "1.5",
            cell: "1.5",
        },
        CellCase {
            label: "a SQL NULL, which carries no text at all",
            selected: "NULL",
            cell: "",
        },
    ];

    for case in &cases {
        let mut request = cluster();
        request["Sql"] = json!(format!(
            "SELECT {} AS value, 1 AS tail",
            case.selected
        ));
        request["ResultFormat"] = json!("CSV");
        let submitted = emulator.execute_and_wait(&request).await;
        let id = id_of(&submitted);

        let fetched = emulator
            .ok("GetStatementResultV2", &json!({"Id": id}))
            .await;
        assert_eq!(
            fetched["Records"],
            json!([{"CSVRecords": format!("value,tail\r\n{},1\r\n", case.cell)}]),
            "case {:?}: the CSV form must carry the header and the row, got {fetched}",
            case.label
        );
        assert_eq!(
            fetched["ResultFormat"], "csv",
            "case {:?}: the reader must report the statement's format, got {fetched}",
            case.label
        );
        assert_eq!(
            number(&fetched, "TotalNumRows"),
            1,
            "case {:?}: the result holds one row, got {fetched}",
            case.label
        );
        let columns = list(&fetched, "ColumnMetadata");
        assert_eq!(
            columns.len(),
            2,
            "case {:?}: the result's two columns must be reported, got {fetched}",
            case.label
        );
        assert_eq!(
            columns[0]["name"], "value",
            "case {:?}: the first column is the selected one, got {fetched}",
            case.label
        );
    }
}

/// One row of the reader table: the format a statement was submitted with, the
/// reader a caller addressed, and what that reader answers.
struct ReaderCase {
    label: &'static str,
    /// The `ResultFormat` the submit sends, or `None` for a submit that sends
    /// none and so takes the documented JSON default.
    submitted: Option<&'static str>,
    sql: &'static str,
    operation: &'static str,
    /// The HTTP status, error code and `Message` the read answers with, or
    /// `None` for a read the service serves.
    refusal: Option<(u16, &'static str, &'static str)>,
}

/// A statement's rows are read by the reader that serves the format the
/// statement was submitted with: the other reader refuses it with the
/// requirement that reader's own documentation states
/// (`smithy_query(query="doc",shape="GetStatementResult")` and
/// `smithy_query(query="doc",shape="GetStatementResultV2")`), and a statement
/// that produced no result at all is refused as a missing result by both.
#[tokio::test]
async fn each_reader_serves_only_the_result_format_it_is_the_reader_for() {
    let emulator = Emulator::new();
    let cases = [
        ReaderCase {
            label: "a CSV statement read by the CSV reader",
            submitted: Some("CSV"),
            sql: "SELECT 1 AS n",
            operation: "GetStatementResultV2",
            refusal: None,
        },
        ReaderCase {
            label: "a CSV statement read by the JSON reader",
            submitted: Some("CSV"),
            sql: "SELECT 1 AS n",
            operation: "GetStatementResult",
            refusal: Some((
                400,
                "ValidationException",
                "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL \
                 statement must have specified ResultFormat as JSON, or let the format default \
                 to JSON.",
            )),
        },
        ReaderCase {
            label: "a JSON statement read by the JSON reader",
            submitted: Some("JSON"),
            sql: "SELECT 1 AS n",
            operation: "GetStatementResult",
            refusal: None,
        },
        ReaderCase {
            label: "a JSON statement read by the CSV reader",
            submitted: Some("JSON"),
            sql: "SELECT 1 AS n",
            operation: "GetStatementResultV2",
            refusal: Some((
                400,
                "ValidationException",
                "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL \
                 statement must have specified ResultFormat as CSV.",
            )),
        },
        ReaderCase {
            label: "a statement submitted with no format read by the CSV reader",
            submitted: None,
            sql: "SELECT 1 AS n",
            operation: "GetStatementResultV2",
            refusal: Some((
                400,
                "ValidationException",
                "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL \
                 statement must have specified ResultFormat as CSV.",
            )),
        },
        ReaderCase {
            label: "a statement submitted with no format read by the JSON reader",
            submitted: None,
            sql: "SELECT 1 AS n",
            operation: "GetStatementResult",
            refusal: None,
        },
        ReaderCase {
            label: "a CSV statement that produced no result set at all",
            submitted: Some("CSV"),
            sql: "CREATE TABLE no_result_csv (id INTEGER)",
            operation: "GetStatementResultV2",
            refusal: Some((
                400,
                "ResourceNotFoundException",
                "Query does not have result. Please check query status with DescribeStatement.",
            )),
        },
    ];

    for case in &cases {
        let mut request = cluster();
        request["Sql"] = json!(case.sql);
        if let Some(submitted) = case.submitted {
            request["ResultFormat"] = json!(submitted);
        }
        let submitted = emulator.execute_and_wait(&request).await;
        let id = id_of(&submitted);

        let (status, response) = emulator.call(case.operation, &json!({"Id": id})).await;
        match case.refusal {
            None => assert_eq!(
                status, 200,
                "case {:?}: the reader must serve the result, got {status} {response}",
                case.label
            ),
            Some((expected_status, code, stated)) => {
                assert_eq!(
                    status, expected_status,
                    "case {:?}: expected {expected_status}, got {status} {response}",
                    case.label
                );
                assert_eq!(
                    error_code(&response),
                    code,
                    "case {:?}: expected {code}, got {response}",
                    case.label
                );
                assert_eq!(
                    message(&response),
                    stated,
                    "case {:?}: expected {stated:?}, got {response}",
                    case.label
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Resolving a submission's named parameters against its SQL
// ---------------------------------------------------------------------------

/// One row of the parameter-resolution table: the statements a submission
/// carries, the `Parameters` it supplies, and the refusal it earns — or `None`
/// for a submission the service admits.
struct ParameterCase {
    label: &'static str,
    sqls: &'static [&'static str],
    /// The `Parameters` the request supplies, or `None` for a request that
    /// supplies the member not at all.
    parameters: Option<Value>,
    refusal: Option<String>,
}

/// The rows every submit operation is held to: what the SQL references and what
/// the request supplies have to be the same set of names, whether the names come
/// from one statement or from every statement of a batch.
fn parameter_cases() -> Vec<ParameterCase> {
    vec![
        ParameterCase {
            label: "the one placeholder the SQL references is supplied",
            sqls: &["SELECT name FROM people WHERE name = :wanted"],
            parameters: Some(json!([{"name": "wanted", "value": "ada"}])),
            refusal: None,
        },
        ParameterCase {
            label: "one placeholder referenced twice is one name",
            sqls: &["SELECT name FROM people WHERE name = :wanted OR name = :wanted"],
            parameters: Some(json!([{"name": "wanted", "value": "ada"}])),
            refusal: None,
        },
        ParameterCase {
            label: "a colon inside a string literal references nothing",
            sqls: &["SELECT ':wanted' AS literal"],
            parameters: None,
            refusal: None,
        },
        ParameterCase {
            label: "a cast's double colon references nothing",
            sqls: &["SELECT 1::INTEGER AS cast_value"],
            parameters: None,
            refusal: None,
        },
        ParameterCase {
            label: "a colon followed by a space references nothing",
            sqls: &["SELECT : one"],
            parameters: Some(json!([{"name": "one", "value": "1"}])),
            refusal: Some(
                "Parameters list contains unused parameters. Unused parameters: [one]".to_string(),
            ),
        },
        ParameterCase {
            label: "a placeholder supplied under another name",
            sqls: &["SELECT name FROM people WHERE id = :i"],
            parameters: Some(json!([{"name": "zzz", "value": "1"}])),
            refusal: Some(
                "Parameters list doesn't contain all parameters. Missing parameters: [i]"
                    .to_string(),
            ),
        },
        ParameterCase {
            label: "one of two placeholders supplied",
            sqls: &["SELECT name FROM people WHERE id = :i AND name = :n"],
            parameters: Some(json!([{"name": "i", "value": "1"}])),
            refusal: Some(
                "Parameters list doesn't contain all parameters. Missing parameters: [n]"
                    .to_string(),
            ),
        },
        ParameterCase {
            label: "neither of two placeholders supplied",
            sqls: &["SELECT name FROM people WHERE id = :i AND name = :n"],
            parameters: Some(json!([{"name": "other", "value": "1"}])),
            refusal: Some(
                "Parameters list doesn't contain all parameters. Missing parameters: [i, n]"
                    .to_string(),
            ),
        },
        ParameterCase {
            label: "a parameter no statement references",
            sqls: &["SELECT 1"],
            parameters: Some(json!([{"name": "x", "value": "1"}])),
            refusal: Some(
                "Parameters list contains unused parameters. Unused parameters: [x]".to_string(),
            ),
        },
        ParameterCase {
            label: "two parameters no statement references, beside one it does",
            sqls: &["SELECT name FROM people WHERE name = :wanted"],
            parameters: Some(json!([
                {"name": "wanted", "value": "ada"},
                {"name": "x", "value": "1"},
                {"name": "y", "value": "2"},
            ])),
            refusal: Some(
                "Parameters list contains unused parameters. Unused parameters: [x, y]".to_string(),
            ),
        },
        ParameterCase {
            label: "the same name supplied twice",
            sqls: &["SELECT name FROM people WHERE id = :i"],
            parameters: Some(json!([
                {"name": "i", "value": "1"},
                {"name": "i", "value": "2"},
            ])),
            refusal: Some("Found duplicate named parameter \"i\"".to_string()),
        },
        ParameterCase {
            label: "a name supplied twice and referenced by nothing",
            sqls: &["SELECT 1"],
            parameters: Some(json!([
                {"name": "x", "value": "1"},
                {"name": "x", "value": "2"},
            ])),
            refusal: Some("Found duplicate named parameter \"x\"".to_string()),
        },
        // A batch's "parameters are available to all SQL statements in the
        // batch. Each statement can reference any subset of the provided
        // parameters", so the same rule holds over the batch AS A WHOLE.
        ParameterCase {
            label: "a batch whose statements reference one supplied name each",
            sqls: &[
                "SELECT name FROM people WHERE name = :first",
                "SELECT name FROM people WHERE name = :second",
            ],
            parameters: Some(json!([
                {"name": "first", "value": "ada"},
                {"name": "second", "value": "alan"},
            ])),
            refusal: None,
        },
        ParameterCase {
            label: "a batch where only one statement references the supplied name",
            sqls: &["SELECT name FROM people WHERE name = :first", "SELECT 1"],
            parameters: Some(json!([{"name": "first", "value": "ada"}])),
            refusal: None,
        },
        ParameterCase {
            label: "a batch referencing a name nothing supplies",
            sqls: &[
                "SELECT name FROM people WHERE name = :first",
                "SELECT name FROM people WHERE name = :second",
            ],
            parameters: Some(json!([{"name": "first", "value": "ada"}])),
            refusal: Some(
                "Parameters list doesn't contain all parameters. Missing parameters: [second]"
                    .to_string(),
            ),
        },
        ParameterCase {
            label: "a batch no statement of which references the supplied name",
            sqls: &["SELECT 1", "SELECT 2"],
            parameters: Some(json!([{"name": "first", "value": "ada"}])),
            refusal: Some(
                "Parameters list contains unused parameters. Unused parameters: [first]"
                    .to_string(),
            ),
        },
    ]
}

/// A submission is admitted only when the names its `Parameters` supply are
/// exactly the named placeholders its SQL references — and a refused submission
/// records no statement at all.
#[tokio::test]
async fn named_parameters_are_resolved_against_the_sql_before_the_submit_is_admitted() {
    let emulator = Emulator::new();
    emulator
        .run("CREATE TABLE people (id INTEGER, name VARCHAR(32))")
        .await;
    emulator
        .run("INSERT INTO people (id, name) VALUES (1, 'ada'), (2, 'alan')")
        .await;
    let mut recorded = list(&emulator.ok("ListStatements", &json!({})).await, "Statements").len();

    for case in parameter_cases() {
        let batch = case.sqls.len() > 1;
        let operation = if batch {
            "BatchExecuteStatement"
        } else {
            "ExecuteStatement"
        };
        let mut request = cluster();
        if batch {
            request["Sqls"] = json!(case.sqls);
        } else {
            request["Sql"] = json!(case.sqls[0]);
        }
        if let Some(parameters) = &case.parameters {
            request["Parameters"] = parameters.clone();
        }

        let (status, response) = emulator.call(operation, &request).await;
        match &case.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {:?}: the submission must be admitted, got {status} {response}",
                    case.label
                );
                recorded += 1;
                let described = emulator
                    .ok("DescribeStatement", &json!({"Id": id_of(&response)}))
                    .await;
                let reported = if batch {
                    json!(list(&described, "SubStatements")
                        .iter()
                        .map(|sub_statement| sub_statement["QueryString"].clone())
                        .collect::<Vec<Value>>())
                } else {
                    json!([described["QueryString"].clone()])
                };
                assert_eq!(
                    reported,
                    json!(case.sqls),
                    "case {:?}: the admitted statements must be recorded, got {described}",
                    case.label
                );
            }
            Some(refusal) => {
                assert_eq!(
                    status, 400,
                    "case {:?}: the submission must be refused, got {status} {response}",
                    case.label
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {:?}: expected ValidationException, got {response}",
                    case.label
                );
                assert_eq!(
                    message(&response),
                    *refusal,
                    "case {:?}: expected {refusal:?}, got {response}",
                    case.label
                );
            }
        }

        let page = emulator.ok("ListStatements", &json!({})).await;
        assert_eq!(
            list(&page, "Statements").len(),
            recorded,
            "case {:?}: expected {recorded} recorded statements, got {page}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// What a replayed ClientToken re-records
// ---------------------------------------------------------------------------

/// One row of the replay table: the request submitted under a token the service
/// has already seen, and the text and parameters the record must report after it.
struct ReplayCase {
    label: &'static str,
    sql: &'static str,
    parameters: Option<Value>,
}

/// Replaying a `ClientToken` answers with the first statement, runs nothing, and
/// re-records the text and parameters of the request that replayed it.
#[tokio::test]
async fn a_replayed_client_token_re_records_the_newly_submitted_request() {
    let emulator = Emulator::new();
    emulator.run("CREATE TABLE marks (label VARCHAR(16))").await;

    let first_sql = "INSERT INTO marks (label) VALUES ('first')";
    let token = "a-token-the-caller-reuses";
    let first = emulator
        .execute_and_wait(&{
            let mut request = cluster();
            request["Sql"] = json!(first_sql);
            request["ClientToken"] = json!(token);
            request
        })
        .await;
    let id = id_of(&first);
    let before = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    assert_eq!(
        before["Status"], "FINISHED",
        "the first submission must have run: {before}"
    );

    let cases = [
        ReplayCase {
            label: "the same SQL again",
            sql: first_sql,
            parameters: None,
        },
        ReplayCase {
            label: "a different SQL",
            sql: "INSERT INTO marks (label) VALUES ('second')",
            parameters: None,
        },
        ReplayCase {
            label: "a different SQL carrying parameters",
            sql: "INSERT INTO marks (label) VALUES (:label)",
            parameters: Some(json!([{"name": "label", "value": "third"}])),
        },
        ReplayCase {
            label: "a different SQL carrying no parameters again",
            sql: "INSERT INTO marks (label) VALUES ('fourth')",
            parameters: None,
        },
    ];

    for case in &cases {
        let mut request = cluster();
        request["Sql"] = json!(case.sql);
        request["ClientToken"] = json!(token);
        if let Some(parameters) = &case.parameters {
            request["Parameters"] = parameters.clone();
        }
        let replayed = emulator.execute_and_wait(&request).await;
        assert_eq!(
            id_of(&replayed),
            id,
            "case {:?}: the token must answer with the first statement, got {replayed}",
            case.label
        );

        let after = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
        assert_eq!(
            after["QueryString"], case.sql,
            "case {:?}: the record must report the newly submitted SQL, got {after}",
            case.label
        );
        let expected_parameters = case.parameters.clone().unwrap_or(Value::Null);
        assert_eq!(
            after["QueryParameters"], expected_parameters,
            "case {:?}: the record must report the newly submitted parameters, got {after}",
            case.label
        );

        // Nothing ran: every value the first execution produced is still the
        // one the record reports.
        for member in ["Status", "Error", "RedshiftQueryId", "RedshiftPid"] {
            assert_eq!(
                after[member], before[member],
                "case {:?}: {member} must stay as the first execution left it, got {after}",
                case.label
            );
        }
        for member in ["ResultRows", "ResultSize", "Duration"] {
            assert_eq!(
                number(&after, member),
                number(&before, member),
                "case {:?}: {member} must stay as the first execution left it, got {after}",
                case.label
            );
        }
        for member in ["CreatedAt", "UpdatedAt"] {
            assert_eq!(
                timestamp(&after, member),
                timestamp(&before, member),
                "case {:?}: {member} must stay frozen at the first execution, got {after}",
                case.label
            );
        }
    }

    // The replays minted no statement, and none of their SQL ever ran: the
    // table still holds only what the FIRST submission wrote.
    let page = emulator.ok("ListStatements", &json!({})).await;
    assert_eq!(
        list(&page, "Statements").len(),
        2,
        "the replays must record no statement of their own, got {page}"
    );
    let read = emulator.run("SELECT label FROM marks ORDER BY label").await;
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": read}))
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"stringValue": "first"}]]),
        "no replayed SQL may have run, got {fetched}"
    );
}

/// The same rule over a batch: replaying a `BatchExecuteStatement`'s token
/// re-records the statements the replay submitted, in their positions, and runs
/// none of them.
#[tokio::test]
async fn a_replayed_batch_token_re_records_the_newly_submitted_statements() {
    let emulator = Emulator::new();
    let token = "a-batch-token-the-caller-reuses";
    let first_sqls = [
        "CREATE TABLE batched (label VARCHAR(16))",
        "INSERT INTO batched (label) VALUES ('first')",
    ];
    let replayed_sqls = [
        "CREATE TABLE batched (label VARCHAR(16))",
        "INSERT INTO batched (label) VALUES ('second')",
    ];

    let mut request = cluster();
    request["Sqls"] = json!(first_sqls);
    request["ClientToken"] = json!(token);
    let first = emulator.batch_and_wait(&request).await;
    let id = id_of(&first);
    let before = emulator.ok("DescribeStatement", &json!({"Id": id})).await;

    let mut replay = cluster();
    replay["Sqls"] = json!(replayed_sqls);
    replay["ClientToken"] = json!(token);
    let second = emulator.batch_and_wait(&replay).await;
    assert_eq!(
        id_of(&second),
        id,
        "the token must answer with the first batch, got {second}"
    );

    let after = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    let recorded: Vec<Value> = list(&after, "SubStatements")
        .iter()
        .map(|sub_statement| sub_statement["QueryString"].clone())
        .collect();
    assert_eq!(
        json!(recorded),
        json!(replayed_sqls),
        "the record must report the newly submitted statements, got {after}"
    );
    assert_eq!(
        after["Status"], before["Status"],
        "the batch's status must stay as its own execution left it, got {after}"
    );
    assert_eq!(
        timestamp(&after, "CreatedAt"),
        timestamp(&before, "CreatedAt"),
        "the batch's CreatedAt must stay frozen at the first execution, got {after}"
    );

    // A replay naming a DIFFERENT number of statements is not a re-submission of
    // the statements the record holds, so the record keeps the text it has: a
    // batch never reports some of its statements from one request and the rest
    // from another.
    let mut shorter = cluster();
    shorter["Sqls"] = json!(["INSERT INTO batched (label) VALUES ('third')"]);
    shorter["ClientToken"] = json!(token);
    let third = emulator.batch_and_wait(&shorter).await;
    assert_eq!(
        id_of(&third),
        id,
        "the token must still answer with the first batch, got {third}"
    );
    let unchanged = emulator.ok("DescribeStatement", &json!({"Id": id})).await;
    let still_recorded: Vec<Value> = list(&unchanged, "SubStatements")
        .iter()
        .map(|sub_statement| sub_statement["QueryString"].clone())
        .collect();
    assert_eq!(
        json!(still_recorded),
        json!(replayed_sqls),
        "a replay of a different number of statements re-records none of them, got {unchanged}"
    );

    // The replayed INSERT never ran, so the table the first batch created holds
    // only the row that batch wrote.
    let read = emulator.run("SELECT label FROM batched").await;
    let fetched = emulator
        .ok("GetStatementResult", &json!({"Id": read}))
        .await;
    assert_eq!(
        fetched["Records"],
        json!([[{"stringValue": "first"}]]),
        "no replayed statement may have run, got {fetched}"
    );
}
