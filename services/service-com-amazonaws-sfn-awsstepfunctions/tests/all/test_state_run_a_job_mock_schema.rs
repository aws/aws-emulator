// SPDX-License-Identifier: CC0-1.0

//! Which API model a MOCKED result is validated against when the state under test is a
//! run-a-job (`.sync`) `Task` — driven over this service's own wire protocol through the
//! production router, so what is asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: `mock.fieldValidationMode` "determines the level of strictness
//! when validating mocked results against their respective API models"
//! (`smithy_query(query="shape",shape="MockInput")`), and the result of a run-a-job task
//! is the JOB's final report — the output of the operation that REPORTS the job, not of
//! the one that started it — so that reporting operation's output is the model such a
//! mocked result is judged by. A member of it arriving as another JSON type is refused
//! 400 `ValidationException` in the service's sentence for a schema defect, naming the
//! member and the type the model declares; the mode `NONE` performs no validation, and a
//! request-response task is judged by its OWN operation's output as before.
//!
//! The harness (`Emulator`, `error_code`) is the one `state_machine_lifecycle` builds;
//! `tests/all/main.rs` is this binary's crate root, so it is reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::{error_code, Emulator, ROLE_ARN};

/// The sentence a mocked result's schema defect is refused in, for the member `field`
/// and the type `must_be` the invoked model declares it as.
///
/// RECORDED text: the fixed clause with the offending member from the mock and the type
/// from the schema it is judged against.
fn schema_refusal(field: &str, must_be: &str) -> String {
    format!("Mock result schema validation error: Field '{field}' must be {must_be}")
}

/// One row of the mocked-report table.
struct ReportCase {
    case: &'static str,
    /// The `Resource` the `Task` state under test names.
    resource: &'static str,
    /// The `mock.result` the request carries, as the caller wrote it.
    result: &'static str,
    /// The `fieldValidationMode` the mock asks for, where it names one.
    mode: Option<&'static str>,
    /// The member and the declared type a refusal names, or `None` where the mocked
    /// result conforms to the model it is judged by.
    refused: Option<(&'static str, &'static str)>,
}

/// A mocked run-a-job result is judged by the output of the operation that reports the
/// job, and a member of another JSON type than that output declares is refused.
///
/// Rows span three run-a-job integrations (a nested execution of this service's own API,
/// an Athena query and a Glue job run), the JSON types their report members are declared
/// as (a structure, a string, an integer), both patterns of the same operation
/// (request-response and `.sync`, which are judged by different models), the pattern's
/// second revision, and the validation mode that performs no validation at all.
#[tokio::test]
async fn a_mocked_run_a_job_result_is_judged_by_the_reporting_operations_output() {
    let cases: [ReportCase; 11] = [
        ReportCase {
            case: "a job run's report member sent as a string",
            resource: "arn:aws:states:::glue:startJobRun.sync",
            result: r#"{"JobRun":"SUCCEEDED"}"#,
            mode: None,
            refused: Some(("JobRun", "an object")),
        },
        ReportCase {
            case: "the same member sent as the object the model declares",
            resource: "arn:aws:states:::glue:startJobRun.sync",
            result: r#"{"JobRun":{"JobRunState":"SUCCEEDED","Id":"jr-1"}}"#,
            mode: None,
            refused: None,
        },
        ReportCase {
            case: "the starting call's own member is no member of the report",
            resource: "arn:aws:states:::glue:startJobRun.sync",
            result: r#"{"JobRunId":7}"#,
            mode: None,
            refused: None,
        },
        ReportCase {
            case: "validating nothing judges the report by nothing",
            resource: "arn:aws:states:::glue:startJobRun.sync",
            result: r#"{"JobRun":"SUCCEEDED"}"#,
            mode: Some("NONE"),
            refused: None,
        },
        ReportCase {
            case: "the mode that judges every present field",
            resource: "arn:aws:states:::glue:startJobRun.sync",
            result: r#"{"JobRun":[]}"#,
            mode: Some("PRESENT"),
            refused: Some(("JobRun", "an object")),
        },
        ReportCase {
            case: "a query's report member sent as a list",
            resource: "arn:aws:states:::athena:startQueryExecution.sync",
            result: r#"{"QueryExecution":[]}"#,
            mode: None,
            refused: Some(("QueryExecution", "an object")),
        },
        ReportCase {
            case: "the same member sent as the object the model declares",
            resource: "arn:aws:states:::athena:startQueryExecution.sync",
            result: r#"{"QueryExecution":{"Status":{"State":"SUCCEEDED"}}}"#,
            mode: None,
            refused: None,
        },
        ReportCase {
            case: "a nested execution's status sent as an object",
            resource: "arn:aws:states:::states:startExecution.sync",
            result: r#"{"ExecutionArn":"arn:aws:states:::execution:m:e","Status":{"of":"it"}}"#,
            mode: None,
            refused: Some(("Status", "a string")),
        },
        ReportCase {
            case: "the second revision of the same integration, with a count as a word",
            resource: "arn:aws:states:::states:startExecution.sync:2",
            result: r#"{"Status":"SUCCEEDED","RedriveCount":"none"}"#,
            mode: None,
            refused: Some(("RedriveCount", "an integer")),
        },
        ReportCase {
            case: "a nested execution's report that conforms",
            resource: "arn:aws:states:::states:startExecution.sync",
            result: r#"{"Status":"SUCCEEDED","Output":"{\"ok\":true}","RedriveCount":0}"#,
            mode: None,
            refused: None,
        },
        ReportCase {
            // The same operation called WITHOUT the pattern answers its own output, so
            // that output is what judges the mocked result: the id the start answers is
            // a string there, and no `JobRun` member exists at all.
            case: "the request-response pattern is judged by the start call's output",
            resource: "arn:aws:states:::aws-sdk:glue:startJobRun",
            result: r#"{"JobRunId":7}"#,
            mode: None,
            refused: Some(("JobRunId", "a string")),
        },
    ];

    let emulator = Emulator::new();
    for ReportCase {
        case,
        resource,
        result,
        mode,
        refused,
    } in cases
    {
        let definition = json!({
            "Type": "Task",
            "Resource": resource,
            "Parameters": { "Name.$": "$.name" },
            "End": true,
        })
        .to_string();
        let mut mock = json!({ "result": result });
        if let Some(mode) = mode {
            mock["fieldValidationMode"] = json!(mode);
        }
        let request = json!({
            "definition": definition,
            "input": r#"{"name":"reported"}"#,
            "roleArn": ROLE_ARN,
            "inspectionLevel": "DEBUG",
            "mock": mock,
        });
        let (status, body) = emulator.call("TestState", &request).await;
        let Some((field, must_be)) = refused else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the mocked report to be tested, got {status} with \
                 {body:?}"
            );
            assert_eq!(
                body.get("status").and_then(Value::as_str),
                Some("SUCCEEDED"),
                "case {case:?}: expected the state to produce the mocked report, got {body:?}"
            );
            continue;
        };
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} with {body:?}"
        );
        let expected = schema_refusal(field, must_be);
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the refusal {expected:?}, got {body:?}"
        );
    }
}
