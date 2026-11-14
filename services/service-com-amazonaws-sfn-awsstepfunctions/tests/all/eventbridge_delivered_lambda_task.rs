// SPDX-License-Identifier: CC0-1.0

//! What an execution an EVENTBRIDGE RULE started makes of a `Task` state whose work is a
//! Lambda function, driven over all three services' own wire protocols through the
//! production `MultiServiceRouter` with Step Functions, Lambda and EventBridge mounted on
//! ONE engine — the shape `core/router/src/lib.rs` mounts them in, reached through
//! [`crate::lambda_task_control_flow::Chain`] so there is one setup of these services in
//! this target — so the event is really routed, the function really runs, and the
//! execution's outcome is that run's own.
//!
//! RULE: a `Task` state whose `Resource` names the Lambda invocation — the optimized
//! `arn:aws:states:::lambda:invoke` integration, the AWS SDK `aws-sdk:lambda:invoke` form,
//! or the function's own ARN — INVOKES that function with the document the state built and
//! reports what the invocation answered, so an execution over such tasks reaches
//! `SUCCEEDED` with no `error` and no `cause`; an integration token's result is the
//! `Invoke` response envelope (`Payload`, `StatusCode`, `ExecutedVersion`) while a function
//! ARN's is the function's own payload alone, and a run that ENDED BY RAISING fails the task
//! under the `errorType` the run's error document names, never under an interpreter-level
//! error such as `States.Runtime`. What the handler is handed is decided by the TARGET: a
//! rule target that overrides nothing starts the execution with the whole event envelope —
//! so `$.id` is the id `PutEvents` minted for the entry, not any id inside `detail` — while a
//! target's own constant `Input` or its `InputPath` starts it with that document instead.
//!
//! The rows differ along what the rule generalises over: the `Resource` form that names the
//! invocation (the optimized integration, the AWS SDK one, the function's own ARN), and the
//! document the target hands the execution (the whole envelope, a constant `Input`, the part
//! an `InputPath` selects) — and with it which id the handler echoes back.
//!
//! Every expected value is computed here from what the row published — the event id out of
//! the `PutEvents` answer, the constant out of the target the row wired — never read back out
//! of the answer being checked.

use std::time::Instant;

use serde_json::{json, Value};

use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, ROLE_ARN};

/// The role an EventBridge rule's state-machine target is started as: "for … Step Functions
/// state machines …, EventBridge relies on IAM roles that you specify in the RoleARN
/// argument in PutTargets" (`PutTargets`), so a target that names none is not accepted at
/// all.
const TARGET_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/EventsInvokeStates";

/// The `detail-type` every event this suite publishes carries, and what each rule matches on
/// beside the row's own `source`.
const DETAIL_TYPE: &str = "Job Requested";

/// The id the published event carries INSIDE its `detail`, which is the one a target handed
/// the whole envelope must NOT give the handler.
const DETAIL_ID: &str = "detail-id-which-is-not-the-envelope-id";

/// The id a target's own constant `Input` carries.
const CONSTANT_ID: &str = "constant-input-id";

/// How long one poll of work the clock owes sleeps between reads, and how long a row waits
/// for a rule's delivery to have started an execution: generous, so a loaded host cannot
/// decide the outcome.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
/// See [`POLL_INTERVAL`].
const DELIVERY_PATIENCE: std::time::Duration =
    PAST_THE_EXECUTION_DELAY.saturating_add(std::time::Duration::from_secs(30));

/// The terminal status an execution whose every task ended reaches, and the one it sits in
/// while a task is still running (`ExecutionStatus`).
const SUCCEEDED: &str = "SUCCEEDED";
const FAILED: &str = "FAILED";

/// The error the Amazon States Language reports for an INTERPRETER-level fault — an
/// unresolvable path, a state whose parameters no call can be built from. A function that
/// raised is not one of those, so no row may see it.
const RUNTIME_ERROR: &str = "States.Runtime";

/// The handler every succeeding row invokes: it echoes the `id` of the document it was
/// handed, so which document the target gave the execution is readable in the task's result.
const ECHO_SOURCE: &str = r#"
def handler(event, context):
    return {"id": event.get("id"), "status": "SUCCEEDED"}
"#;

/// The handler the failing row invokes, which ends by RAISING: its exception class is what
/// the run's error document names as the `errorType`, and so what the failed task reports.
const RAISING_SOURCE: &str = r#"
class SubmitJobUnavailable(Exception):
    pass


def handler(event, context):
    raise SubmitJobUnavailable("the job service could not be reached")
"#;

/// The class that handler raises, and the message it carries.
const RAISED_ERROR_TYPE: &str = "SubmitJobUnavailable";
/// See [`RAISED_ERROR_TYPE`].
const RAISED_MESSAGE: &str = "the job service could not be reached";

/// `definition` as a state machine; answers the ARN Step Functions reported for it.
async fn given_machine(chain: &Chain, case: &str, name: &str, definition: &Value) -> String {
    let created = chain
        .sfn(
            case,
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": definition.to_string(),
                "roleArn": ROLE_ARN,
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// A rule on the default bus matching `source`, with `machine` as its one target and the
/// row's own override on that target, then the event the row publishes; answers the id
/// `PutEvents` minted for that event.
async fn given_delivery(
    chain: &Chain,
    case: &str,
    name: &str,
    source: &str,
    machine: &str,
    input: Option<&str>,
    input_path: Option<&str>,
) -> String {
    chain
        .events(
            case,
            "PutRule",
            &json!({
                "Name": name,
                "EventPattern": json!({
                    "source": [source],
                    "detail-type": [DETAIL_TYPE],
                })
                .to_string(),
                "State": "ENABLED",
            }),
        )
        .await;
    let mut target = json!({
        "Id": "to-state-machine",
        "Arn": machine,
        "RoleArn": TARGET_ROLE_ARN,
    });
    if let Some(constant) = input {
        target["Input"] = Value::String(constant.to_string());
    }
    if let Some(path) = input_path {
        target["InputPath"] = Value::String(path.to_string());
    }
    let wired = chain
        .events(
            case,
            "PutTargets",
            &json!({ "Rule": name, "Targets": [target] }),
        )
        .await;
    assert_eq!(
        wired.get("FailedEntryCount").and_then(Value::as_i64),
        Some(0),
        "case {case:?}: the target must be accepted, got {wired}"
    );
    let published = chain
        .events(
            case,
            "PutEvents",
            &json!({
                "Entries": [{
                    "Source": source,
                    "DetailType": DETAIL_TYPE,
                    "Detail": json!({ "id": DETAIL_ID, "job": "nightly" }).to_string(),
                }],
            }),
        )
        .await;
    assert_eq!(
        published.get("FailedEntryCount").and_then(Value::as_i64),
        Some(0),
        "case {case:?}: the event must be accepted, got {published}"
    );
    published
        .pointer("/Entries/0/EventId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: PutEvents answers an EventId, got {published}"))
        .to_string()
}

/// The ARN of the one execution of `machine`, looked for until the delivery that starts it
/// has been made.
async fn delivered_execution(chain: &Chain, case: &str, machine: &str) -> String {
    let began = Instant::now();
    loop {
        let listed = chain
            .sfn(
                case,
                "ListExecutions",
                &json!({ "stateMachineArn": machine }),
            )
            .await;
        let executions = listed
            .get("executions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let [only] = executions {
            return string(only, "executionArn");
        }
        assert!(
            executions.is_empty(),
            "case {case:?}: the rule's one delivery must start one execution, got {listed}"
        );
        assert!(
            began.elapsed() < DELIVERY_PATIENCE,
            "case {case:?}: the delivery must have started an execution within \
             {DELIVERY_PATIENCE:?}, last saw {listed}"
        );
        // Awaited rather than blocking: the delivery this loop waits for is a task of the
        // same runtime.
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The document an execution's `output` carries.
fn output_of(case: &str, described: &Value) -> Value {
    serde_json::from_str(described.get("output").and_then(Value::as_str).unwrap_or_else(
        || panic!("case {case:?}: a succeeded execution reports an output, got {described}"),
    ))
    .unwrap_or_else(|error| {
        panic!("case {case:?}: the output is a JSON document, got {error} in {described}")
    })
}

/// The `Resource` form a row's one `Task` state names the invocation in.
#[derive(Clone, Copy)]
enum Work {
    /// An integration token, whose request is the `Parameters` the state builds and whose
    /// result is the `Invoke` response envelope.
    Integration(&'static str),
    /// The function's own ARN, whose event is the state's effective input and whose result
    /// is the function's own payload alone.
    FunctionArn,
}

/// Where the id the handler echoes must come from.
#[derive(Clone, Copy)]
enum Echoed {
    /// The id `PutEvents` minted for the entry, which is the envelope's own `id`.
    Envelope,
    /// The id the event carries inside its `detail`.
    Detail,
    /// The id the target's own constant `Input` carries.
    Constant,
}

/// One row: how the task names its work, what the target overrides, and which id the
/// handler must therefore echo.
struct Row {
    case: &'static str,
    source: &'static str,
    work: Work,
    input: Option<String>,
    input_path: Option<&'static str>,
    echoed: Echoed,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a target that overrides nothing, through the optimized integration",
            source: "sim2real.envelope.optimized",
            work: Work::Integration("arn:aws:states:::lambda:invoke"),
            input: None,
            input_path: None,
            echoed: Echoed::Envelope,
        },
        Row {
            case: "the AWS SDK family of the same invocation",
            source: "sim2real.envelope.awssdk",
            work: Work::Integration("arn:aws:states:::aws-sdk:lambda:invoke"),
            input: None,
            input_path: None,
            echoed: Echoed::Envelope,
        },
        Row {
            case: "the function's own ARN as the Resource",
            source: "sim2real.envelope.function-arn",
            work: Work::FunctionArn,
            input: None,
            input_path: None,
            echoed: Echoed::Envelope,
        },
        Row {
            case: "a target with its own constant Input, which replaces the envelope",
            source: "sim2real.envelope.constant-input",
            work: Work::Integration("arn:aws:states:::lambda:invoke"),
            input: Some(json!({ "id": CONSTANT_ID }).to_string()),
            input_path: None,
            echoed: Echoed::Constant,
        },
        Row {
            case: "a target whose InputPath selects the detail out of the envelope",
            source: "sim2real.envelope.input-path",
            work: Work::Integration("arn:aws:states:::lambda:invoke"),
            input: None,
            input_path: Some("$.detail"),
            echoed: Echoed::Detail,
        },
    ]
}

/// A one-`Task` poller whose work is the function `function_arn`, named as `work` names it.
fn definition(work: Work, function_arn: &str) -> Value {
    let state = match work {
        Work::Integration(resource) => json!({
            "Type": "Task",
            "Resource": resource,
            "Parameters": { "FunctionName": function_arn, "Payload.$": "$" },
            "End": true,
        }),
        Work::FunctionArn => json!({
            "Type": "Task",
            "Resource": function_arn,
            "End": true,
        }),
    };
    json!({ "StartAt": "SubmitJob", "States": { "SubmitJob": state } })
}

/// Every row: the rule's delivery starts an execution, the `Task` state really invokes the
/// function, and the execution reaches `SUCCEEDED` reporting what the run answered about the
/// document the target handed it.
#[tokio::test]
async fn an_eventbridge_delivered_lambda_task_runs_on_the_document_the_target_hands_it() {
    let chain = Chain::with_eventbridge();
    let function_arn = chain
        .given_function_of("echo", ECHO_SOURCE, None)
        .await;

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let machine = given_machine(
            &chain,
            case,
            &format!("delivered-{index}"),
            &definition(row.work, &function_arn),
        )
        .await;
        let event_id = given_delivery(
            &chain,
            case,
            &format!("delivered-rule-{index}"),
            row.source,
            &machine,
            row.input.as_deref(),
            row.input_path,
        )
        .await;
        let expected = match row.echoed {
            Echoed::Envelope => event_id.clone(),
            Echoed::Detail => DETAIL_ID.to_string(),
            Echoed::Constant => CONSTANT_ID.to_string(),
        };

        let execution = delivered_execution(&chain, case, &machine).await;
        let described = chain.settled(case, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(SUCCEEDED),
            "case {case:?}: the delivered execution must run the poller to completion, got \
             {described}"
        );
        assert_eq!(
            (described.get("error"), described.get("cause")),
            (None, None),
            "case {case:?}: a run that succeeds carries neither error nor cause, got {described}"
        );

        // An integration token's result is the `Invoke` response envelope, so the
        // function's own answer sits under `Payload`; a function ARN's result is that
        // answer alone.
        let output = output_of(case, &described);
        let answered = match row.work {
            Work::Integration(_) => {
                assert_eq!(
                    output.get("StatusCode").and_then(Value::as_i64),
                    Some(200),
                    "case {case:?}: the envelope reports the invocation's status, got {output}"
                );
                assert_eq!(
                    output.get("ExecutedVersion").and_then(Value::as_str),
                    Some("$LATEST"),
                    "case {case:?}: the envelope reports the version that ran, got {output}"
                );
                output
                    .get("Payload")
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the envelope carries a Payload, got {output}")
                    })
                    .clone()
            }
            Work::FunctionArn => {
                assert_eq!(
                    output.get("StatusCode"),
                    None,
                    "case {case:?}: a function ARN's result is the output alone, got {output}"
                );
                output.clone()
            }
        };
        assert_eq!(
            answered.get("id").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the handler to echo the id {expected:?} out of the \
             document the target handed the execution, got {answered}"
        );
        assert_eq!(
            answered.get("status").and_then(Value::as_str),
            Some(SUCCEEDED),
            "case {case:?}: the task's result is the function's own answer, got {answered}"
        );
    }
}

/// A delivered execution whose handler ends by RAISING fails under the error document the
/// run produced — the function's own error class and that document as the cause — rather
/// than under the interpreter's own [`RUNTIME_ERROR`]. What the failure mapping IS is pinned
/// over several runtimes and error classes by `unhandled_handler_error_task_failure`; what
/// this case adds is that a RULE-DELIVERED execution reports the same thing, since the
/// emulator used to end such a run on an unsupported-integration fault instead.
#[tokio::test]
async fn a_delivered_lambda_task_whose_handler_raises_reports_the_functions_own_error() {
    let case = "a delivered poller whose handler raises";
    let chain = Chain::with_eventbridge();
    let function_arn = chain
        .given_function_of("raiser", RAISING_SOURCE, None)
        .await;
    let machine = given_machine(
        &chain,
        case,
        "delivered-raiser",
        &definition(
            Work::Integration("arn:aws:states:::lambda:invoke"),
            &function_arn,
        ),
    )
    .await;
    given_delivery(
        &chain,
        case,
        "delivered-raiser-rule",
        "sim2real.envelope.raiser",
        &machine,
        None,
        None,
    )
    .await;

    let execution = delivered_execution(&chain, case, &machine).await;
    let described = chain.settled(case, &execution).await;
    assert_eq!(
        described.get("status").and_then(Value::as_str),
        Some(FAILED),
        "case {case:?}: an invocation whose handler raised is a failed unit of work, got \
         {described}"
    );
    let error = described.get("error").and_then(Value::as_str);
    assert_ne!(
        error,
        Some(RUNTIME_ERROR),
        "case {case:?}: {RUNTIME_ERROR} is reserved for an interpreter-level fault, got \
         {described}"
    );
    assert_eq!(
        error,
        Some(RAISED_ERROR_TYPE),
        "case {case:?}: expected the error class the run's document names, got {described}"
    );
    let cause: Value = serde_json::from_str(
        described.get("cause").and_then(Value::as_str).unwrap_or_else(|| {
            panic!("case {case:?}: a failed run carries a cause, got {described}")
        }),
    )
    .unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the cause is the run's own error document, got {error} in {described}"
        )
    });
    assert_eq!(
        cause.get("errorType").and_then(Value::as_str),
        Some(RAISED_ERROR_TYPE),
        "case {case:?}: the cause names the error class, got {cause}"
    );
    assert_eq!(
        cause.get("errorMessage").and_then(Value::as_str),
        Some(RAISED_MESSAGE),
        "case {case:?}: the cause carries the message the handler raised, got {cause}"
    );
}
