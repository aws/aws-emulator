// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the diagnosis a document past what a state's output may carry
//! is reported with, EVERYWHERE a client reads it: `DescribeExecution`, the failure
//! events of `GetExecutionHistory`, and the error output a `Catch` handler is handed.
//! Driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions, S3 and IAM mounted on ONE engine.
//!
//! The rules under test:
//!
//! 1. a `Task` state whose result is larger than a state's output may carry fails the
//!    execution `States.DataLimitExceeded` with the service's own quota sentence, whose
//!    ONE variable part is the KIND of resource the call was made on
//!    (`aws-sdk:<service>` for the AWS SDK family) — the same naming that state's
//!    history events carry — and never the state's name, the action, the whole
//!    `Resource` ARN or the number of bytes the result happened to be; and the task's
//!    failure event and the execution's terminal event repeat that error and that cause
//!    unchanged, which is where a client's error handling reads them;
//! 2. a document a state built ITSELF past that bound carries the same sentence naming
//!    THAT state, wherever the service reports it — the execution's own cause where the
//!    failure ended the execution, and the error output a catcher of that state was
//!    handed where it took the failure over.
//!
//! Each table's rows differ along what its rule generalises over: which service answered
//! the oversize result, which ACTION of that service was called and what the calling
//! state is NAMED (rule 1), and which state type built the document and whether that
//! state declares a `Catch` (rule 2).
//!
//! The harness is `crate::map_result_writer`'s `Chain`, as
//! `crate::s3_sdk_task_object_lifecycle` uses it: `tests/all.rs` is this binary's crate
//! root, so this suite drives the same three services through it.

use serde_json::{json, Map, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::asl::{DATA_LIMIT_ERROR, MAX_PAYLOAD_BYTES};

use crate::map_result_writer::{every_action, Chain, BUCKET};

/// What the service states about a result larger than a state's output may carry:
/// RECORDED text, with the work that produced it named in it.
fn too_large(produced_by: &str) -> String {
    format!(
        "The state/task '{produced_by}' returned a result with a size exceeding the maximum \
         number of bytes service limit."
    )
}

/// The object whose bytes are past what a state's output may carry.
const OVERSIZE_KEY: &str = "diagnosis/oversize-object";

/// A definition whose one `Task` state, named `state`, makes `resource`'s call with
/// `parameters` and ends with whatever it answered.
fn calling(state: &str, resource: &str, parameters: Value) -> Value {
    json!({
        "StartAt": state,
        "States": {
            state: {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
}

/// The events of one execution, in the order the history reports them.
async fn events_of(chain: &Chain, execution_arn: &str, case: &str) -> Vec<Value> {
    let answered = chain
        .sfn(
            "GetExecutionHistory",
            &json!({ "executionArn": execution_arn }),
        )
        .await;
    answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a history, got {answered:?}"))
}

/// Every event type a history carries, for a failed assertion to report.
fn event_types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .collect()
}

/// The details of the ONE event of type `event_type` the history carries.
fn only_detail(events: &[Value], event_type: &str, details: &str, case: &str) -> Value {
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some(event_type))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one {event_type} event, got {:?}",
        event_types(events)
    );
    matching[0]
        .get(details)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected {details} on {:?}", matching[0]))
}

/// The `error` and the `cause` one record — a described execution or an event's details
/// — reports, as the text each holds.
fn failure(record: &Value) -> (Option<&str>, Option<&str>) {
    (
        record.get("error").and_then(Value::as_str),
        record.get("cause").and_then(Value::as_str),
    )
}

// ---------------------------------------------------------------------------
// Rule 1: an oversize TASK result names the kind of resource it came from
// ---------------------------------------------------------------------------

/// An oversize `Task` result fails the execution with the quota sentence naming the KIND
/// of resource the call was made on, and the task's failure event and the execution's
/// terminal event carry that same error and cause — so a client reading either reads the
/// one sentence.
///
/// The rows differ along which service answered the oversize result, which action of it
/// was called, and what the calling state is named: the two rows of each service must
/// report the identical sentence, which is what pins the state's name, the action and
/// the observed size OUT of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversize_task_result_reports_one_sentence_wherever_it_is_read() {
    // A body past the bound a state's output carries, by enough that no envelope around
    // it can bring the document back inside.
    let oversize = "p".repeat(MAX_PAYLOAD_BYTES + 1024);

    let chain = Chain::new();
    chain.bucket(BUCKET).await;
    let role_arn = chain.role("OversizeDiagnosisRole", &every_action()).await;
    chain.object(OVERSIZE_KEY, &oversize).await;

    // A state machine whose own definition is past the bound, so every read of that
    // definition is a result past it too. Its `Comment` is where the language lets a
    // definition carry text of any length.
    let described_machine = chain
        .machine(
            "oversize-diagnosis-definition",
            &json!({
                "Comment": oversize,
                "StartAt": "Done",
                "States": { "Done": { "Type": "Succeed" } },
            }),
            &role_arn,
        )
        .await;
    // An execution of it, so the SECOND reader of that definition — the one that reads
    // it off an execution — has an execution to be asked about. Its own run carries no
    // oversize document, so it succeeds.
    let own_case = "the run of the machine whose definition is oversize";
    let (described_execution, described) = chain.ran(&described_machine, &json!({}), own_case).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some("SUCCEEDED"),
        "case {own_case:?}: a definition past the bound is not itself a state's output, \
         got {described:?}"
    );

    // (case, the state's name, the Resource, its parameters, the work the cause must
    // name, the action that work's own events name)
    let rows: &[(&str, &str, &str, Value, &str, &str)] = &[
        (
            "an object S3 answers",
            "Get",
            "arn:aws:states:::aws-sdk:s3:getObject",
            json!({ "Bucket": BUCKET, "Key": OVERSIZE_KEY }),
            "aws-sdk:s3",
            "getObject",
        ),
        (
            "the same object, under another state name",
            "FetchTheObjectFromItsBucket",
            "arn:aws:states:::aws-sdk:s3:getObject",
            json!({ "Bucket": BUCKET, "Key": OVERSIZE_KEY }),
            "aws-sdk:s3",
            "getObject",
        ),
        (
            "a definition Step Functions answers",
            "Describe",
            "arn:aws:states:::aws-sdk:sfn:describeStateMachine",
            json!({ "StateMachineArn": described_machine }),
            "aws-sdk:sfn",
            "describeStateMachine",
        ),
        (
            "the same definition through another action of that service",
            "DescribeForExecution",
            "arn:aws:states:::aws-sdk:sfn:describeStateMachineForExecution",
            json!({ "ExecutionArn": described_execution }),
            "aws-sdk:sfn",
            "describeStateMachineForExecution",
        ),
    ];

    for (index, (case, state, resource, parameters, produced_by, action)) in
        rows.iter().enumerate()
    {
        let definition = calling(state, resource, parameters.clone());
        let machine = chain
            .machine(
                &format!("oversize-diagnosis-{index}"),
                &definition,
                &role_arn,
            )
            .await;
        let (execution, described) = chain.ran(&machine, &json!({}), case).await;
        let expected = too_large(produced_by);

        // What a client polling the execution reads.
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            failure(&described),
            (Some(DATA_LIMIT_ERROR), Some(expected.as_str())),
            "case {case:?}: expected the quota sentence naming {produced_by:?}, got {described:?}"
        );
        assert!(
            described.get("output").is_none(),
            "case {case:?}: an execution that failed puts out nothing, got {described:?}"
        );

        // What a client reading the history reads: the task's own failure, naming the
        // work exactly as the cause names it, and then the execution's terminal event
        // repeating both unchanged.
        let events = events_of(&chain, &execution, case).await;
        let task_failed = only_detail(&events, "TaskFailed", "taskFailedEventDetails", case);
        assert_eq!(
            (
                task_failed.get("resourceType").and_then(Value::as_str),
                task_failed.get("resource").and_then(Value::as_str),
            ),
            (Some(*produced_by), Some(*action)),
            "case {case:?}: expected the failure to name {produced_by}/{action}, got \
             {task_failed:?}"
        );
        assert_eq!(
            failure(&task_failed),
            (Some(DATA_LIMIT_ERROR), Some(expected.as_str())),
            "case {case:?}: expected the task's failure to carry the quota sentence, got \
             {task_failed:?}"
        );
        let execution_failed = only_detail(
            &events,
            "ExecutionFailed",
            "executionFailedEventDetails",
            case,
        );
        assert_eq!(
            failure(&execution_failed),
            (Some(DATA_LIMIT_ERROR), Some(expected.as_str())),
            "case {case:?}: expected the execution's failure to repeat the task's, got \
             {execution_failed:?}"
        );
        // Nothing succeeded for the state, and nothing it would have put out was
        // recorded: the result never became this state's output.
        assert_eq!(
            event_types(&events)
                .into_iter()
                .filter(|kind| *kind == "TaskSucceeded" || *kind == "TaskStateExited")
                .count(),
            0,
            "case {case:?}: expected no success for the state, got {:?}",
            event_types(&events)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a document a state built itself names THAT state, wherever it is read
// ---------------------------------------------------------------------------

/// How many documents each row's state builds, and how large each one is: eight
/// documents of this size are each well inside what a state's output may carry and are
/// past it together, so what trips the bound is the state's whole result rather than any
/// one document of it.
const PRODUCED_COUNT: usize = 8;
const PRODUCED_BYTES: usize = 40 * 1024;

/// The input every row is started with: the document each unit of work copies, and the
/// items a `Map` state iterates.
fn growing_input() -> Value {
    json!({
        "blob": "b".repeat(PRODUCED_BYTES),
        "items": (0..PRODUCED_COUNT).collect::<Vec<usize>>(),
    })
}

/// A definition whose one state, named `name`, is `state` — with a catcher of every
/// error and the state it routes to when `catching`.
fn one_state(name: &str, mut state: Value, catching: bool) -> Value {
    let mut states = Map::new();
    if catching {
        state["Catch"] = json!([{
            "ErrorEquals": ["States.ALL"],
            "ResultPath": "$.caught",
            "Next": "Done",
        }]);
        states.insert("Done".to_string(), json!({ "Type": "Pass", "End": true }));
    }
    state["End"] = json!(true);
    states.insert(name.to_string(), state);
    json!({ "StartAt": name, "States": Value::Object(states) })
}

/// A `Map` state whose iterations each copy the input's document, so the array they
/// build together is past what a state's output may carry.
fn iterating(name: &str, catching: bool) -> Value {
    let state = json!({
        "Type": "Map",
        "ItemsPath": "$.items",
        "ItemSelector": { "b.$": "$.blob" },
        "ItemProcessor": {
            "StartAt": "Take",
            "States": { "Take": { "Type": "Pass", "End": true } },
        },
    });
    one_state(name, state, catching)
}

/// A `Parallel` state whose branches each select the input's document, so the array they
/// build together is past what a state's output may carry.
fn branching(name: &str, catching: bool) -> Value {
    let branch = json!({
        "StartAt": "Take",
        "States": { "Take": { "Type": "Pass", "OutputPath": "$.blob", "End": true } },
    });
    let state = json!({
        "Type": "Parallel",
        "Branches": vec![branch; PRODUCED_COUNT],
    });
    one_state(name, state, catching)
}

/// The error and the cause the service REPORTED for the document that outgrew the bound:
/// the execution's own where the failure ended the execution, and the error output the
/// state's catcher was handed where it took the failure over. Which of the two a
/// definition that declares a `Catch` is given is the service's to decide; the sentence
/// has to name the state that built the document either way.
fn reported(described: &Value, case: &str) -> (String, String) {
    if let (Some(error), Some(cause)) = failure(described) {
        return (error.to_string(), cause.to_string());
    }
    let output: Value = described
        .get("output")
        .and_then(Value::as_str)
        .map(|text| {
            serde_json::from_str(text)
                .unwrap_or_else(|error| panic!("case {case:?}: the output is JSON: {error}"))
        })
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a failure or an error output, got {described:?}")
        });
    let caught = output.pointer("/caught").cloned().unwrap_or_else(|| {
        panic!("case {case:?}: expected the catcher's error output, got {output:?}")
    });
    (
        caught
            .get("Error")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        caught
            .get("Cause")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// A state whose own result outgrows what a state's output may carry is reported with
/// the quota sentence naming THAT state, whether the failure ends the execution or a
/// catcher of the state takes it over.
///
/// The rows differ along which state type built the document — a `Map` state's
/// iterations and a `Parallel` state's branches, each of which grows its array as it runs
/// — and along whether the state declares a `Catch`, which is what decides WHERE the
/// cause is read from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_document_a_state_built_past_the_bound_names_that_state_wherever_it_is_read() {
    let chain = Chain::new();
    let role_arn = chain.role("OversizeProducerRole", &every_action()).await;
    let input = growing_input();

    // (case, the state's name, its definition, whether it catches its own failure)
    let rows: &[(&str, &str, Value, bool)] = &[
        (
            "a 'Map' state's iterations",
            "Fan",
            iterating("Fan", false),
            false,
        ),
        (
            "a 'Map' state's iterations, under a state that catches",
            "FanAndCatch",
            iterating("FanAndCatch", true),
            true,
        ),
        (
            "a 'Parallel' state's branches",
            "Fork",
            branching("Fork", false),
            false,
        ),
        (
            "a 'Parallel' state's branches, under a state that catches",
            "ForkAndCatch",
            branching("ForkAndCatch", true),
            true,
        ),
    ];

    for (index, (case, state, definition, catching)) in rows.iter().enumerate() {
        let machine = chain
            .machine(
                &format!("oversize-producer-{index}"),
                definition,
                &role_arn,
            )
            .await;
        let (_execution, described) = chain.ran(&machine, &input, case).await;
        assert_eq!(
            reported(&described, case),
            (DATA_LIMIT_ERROR.to_string(), too_large(state)),
            "case {case:?}: expected the quota sentence naming {state:?} (catching: \
             {catching}), got {described:?}"
        );
    }
}
