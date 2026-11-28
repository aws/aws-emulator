// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ATTEMPT a state reads out of the context object (`$$`),
//! driven over this service's own wire protocol (`awsJson1_0`, `POST /` with
//! `X-Amz-Target: AWSStepFunctions.<Operation>`) through the production
//! `MultiServiceRouter`.
//!
//! The rule under test: for a state of a kind the Amazon States Language gives a `Retry`
//! — the kinds something can re-run — the context object's `State` carries `RetryCount`,
//! the number of retries that state has already taken: zero on the first attempt, and one
//! more on each attempt a retrier of its own `Retry` queues. So a field whose value is
//! `$$.State.RetryCount` resolves on EVERY attempt, the state machine carries on, and the
//! document each attempt hands its work reports the attempt that built it.
//!
//! Rows vary what the rule generalises over: the KIND of the state being retried (`Task`,
//! `Parallel`, `Map`), the form a `Task` state's `Resource` takes (a function ARN, the
//! optimized integration, an activity a worker claims), the query language the member is
//! read in (JSONPath's `$$` and JSONata's `$states.context`), and how many retries the
//! work needs before it succeeds (none, one, two, three).
//!
//! The harnesses are the ones the sibling suites build: `Chain`
//! (`lambda_task_control_flow`) mounts Step Functions beside Lambda on one engine, so a
//! retried `Task` state's work is a function that really runs; `Emulator`
//! (`state_machine_lifecycle`) serves this service alone, which is all a `Parallel`, a
//! `Map` and an activity task need. `tests/all/main.rs` picks this file up, so each is
//! reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::activity_tasks::{activity_definition, claimed, create_activity};
use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::{
    create_machine, describe_execution, start_execution, string, Emulator,
};
use crate::task_work_event_families::history;

/// The state every definition here reads the member in, named as the recorded case names
/// the retried task.
const STATE_NAME: &str = "InvokeRetry";

/// The member the rule is about.
const ATTEMPT_MEMBER: &str = "RetryCount";

/// What the states below carry the attempt they read into, so one reader serves every row.
const ATTEMPT_FIELD: &str = "retryCount";

/// The JSONPath a JSONPath state reads the member through, and the reference path a
/// `Choice` rule then tests the field it was carried into by.
const ATTEMPT_PATH: &str = "$$.State.RetryCount";
/// See [`ATTEMPT_PATH`].
const ATTEMPT_FIELD_PATH: &str = "$.retryCount";

/// The status a run that resolved the member on every attempt reports.
const SUCCEEDED: &str = "SUCCEEDED";

/// How often an execution is looked at again, and how long it is waited for at all.
///
/// The wait is for the CONDITION — the execution leaving `RUNNING` — so the deadline is
/// generous and a host that settles one quickly spends none of it: nothing here measures
/// how fast the host runs.
const POLL: Duration = Duration::from_millis(25);
/// See [`POLL`].
const DEADLINE: Duration = Duration::from_secs(120);

/// The exception the retried function raises while it has not been retried enough times,
/// whose class is the `errorType` of the error document the invocation produces — and so
/// the name the failed task reports, which the state's own retrier names.
const TOO_EARLY: &str = "TooEarly";

/// The handler the retried `Task` rows invoke: it reads the attempt the state handed it
/// and FAILS while that attempt is below the one the row asks it to succeed on, so what
/// decides the outcome of each attempt is the number the context object supplied.
const RETRY_HANDLER: &str = r#"
class TooEarly(Exception):
    pass


def handler(event, context):
    attempt = int(event["retryCount"])
    succeed_on = int(event["succeedOn"])
    if attempt < succeed_on:
        raise TooEarly("attempt %d has not reached %d" % (attempt, succeed_on))
    return {"retryCount": attempt, "succeedOn": succeed_on}
"#;

/// The `Retry` every retried state here declares: one retrier with room for every row's
/// attempts, growing its interval by nothing so a row waits no longer than the language's
/// own default interval per retry.
///
/// `ErrorEquals` is the language's own `States.ALL` wildcard for the rows whose failure is
/// a state of the definition's own, and the raised class for the rows whose failure is the
/// function's.
fn retry_of(error: &str) -> Value {
    json!([{
        "ErrorEquals": [error],
        "IntervalSeconds": 1,
        "BackoffRate": 1.0,
        "MaxAttempts": 4,
    }])
}

/// The retries the rows ask for: none (the first attempt, which has been retried no
/// times), and then one more per row.
const RETRY_COUNTS: [u64; 4] = [0, 1, 2, 3];

/// The attempts a state that succeeded on its `retries`th retry read, in order.
fn attempts_up_to(retries: u64) -> Vec<u64> {
    (0..=retries).collect()
}

// ---------------------------------------------------------------------------
// A retried `Task` state, whose work is a function that really runs
// ---------------------------------------------------------------------------

/// One `Task` row: the form the state's `Resource` takes, and where the history records
/// the document each attempt handed the work.
struct TaskForm {
    case: &'static str,
    /// The state, over the function's ARN and the attempt the row succeeds on.
    state: fn(&str, u64) -> Value,
    /// The event the hand-over of one attempt is recorded as.
    scheduled: &'static str,
    /// The detail member that event carries, and the member of it holding the document.
    details: &'static str,
    document: &'static str,
    /// The member of that document the payload sits under, where the form nests it.
    payload: Option<&'static str>,
}

/// A `Task` state whose `Resource` is the function's ARN: the document the state builds
/// IS the invocation's event, and the document the function returned is the state's
/// result.
fn function_arn_state(function: &str, succeed_on: u64) -> Value {
    json!({
        "Type": "Task",
        "Resource": function,
        "Parameters": {
            "retryCount.$": ATTEMPT_PATH,
            "succeedOn": succeed_on,
        },
        "Retry": retry_of(TOO_EARLY),
        "End": true,
    })
}

/// The same work through the optimized integration, which names the function in its own
/// `Parameters` and answers the whole SDK response: the row's `ResultSelector` reads the
/// returned document out of that response, so both forms report the same result.
fn optimized_state(function: &str, succeed_on: u64) -> Value {
    json!({
        "Type": "Task",
        "Resource": "arn:aws:states:::lambda:invoke",
        "Parameters": {
            "FunctionName": function,
            "Payload": {
                "retryCount.$": ATTEMPT_PATH,
                "succeedOn": succeed_on,
            },
        },
        "ResultSelector": {
            "retryCount.$": "$.Payload.retryCount",
            "succeedOn.$": "$.Payload.succeedOn",
        },
        "Retry": retry_of(TOO_EARLY),
        "End": true,
    })
}

fn task_forms() -> Vec<TaskForm> {
    vec![
        TaskForm {
            case: "a function ARN, whose invocation is handed the state's own document",
            state: function_arn_state,
            scheduled: "LambdaFunctionScheduled",
            details: "lambdaFunctionScheduledEventDetails",
            document: "input",
            payload: None,
        },
        TaskForm {
            case: "the optimized integration, which carries the document under its payload",
            state: optimized_state,
            scheduled: "TaskScheduled",
            details: "taskScheduledEventDetails",
            document: "parameters",
            payload: Some("Payload"),
        },
    ]
}

/// The attempts a run's history says were handed over, in the order the events record
/// them: the number each attempt's own document carried.
fn attempts_handed_over(case: &str, events: &[Value], form: &TaskForm) -> Vec<u64> {
    let mut attempts = Vec::new();
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some(form.scheduled) {
            continue;
        }
        let details = event
            .get(form.details)
            .unwrap_or_else(|| panic!("case {case:?}: expected {} on {event}", form.details));
        let text = details
            .get(form.document)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {} on {details}", form.document)
            });
        let document: Value = serde_json::from_str(text)
            .unwrap_or_else(|error| panic!("case {case:?}: expected JSON, got {error} for {text:?}"));
        let handed = match form.payload {
            Some(member) => document.get(member).cloned().unwrap_or(Value::Null),
            None => document.clone(),
        };
        attempts.push(
            handed
                .get(ATTEMPT_FIELD)
                .and_then(Value::as_u64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected the attempt in {handed}, got none")
                }),
        );
    }
    attempts
}

/// A retried `Task` state reads the attempt it is on out of the context object: the work
/// of each attempt is handed the number of retries already taken, so a definition whose
/// work succeeds only on a later attempt runs to `SUCCEEDED`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retried_task_state_is_handed_the_attempt_it_is_on() {
    let chain = Chain::new();
    let function = chain
        .given_function_of("retry-count-task", RETRY_HANDLER, None)
        .await;

    for form in task_forms() {
        for retries in RETRY_COUNTS {
            let case = format!("{} succeeding on retry {retries}", form.case);
            let case = case.as_str();
            let definition = json!({
                "StartAt": STATE_NAME,
                "States": { STATE_NAME: (form.state)(&function, retries) },
            });
            let described = chain
                .ran(
                    case,
                    &format!("retry-count-{}-{retries}", form.scheduled),
                    &definition,
                    &json!({}),
                )
                .await;

            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some(SUCCEEDED),
                "case {case:?}: expected the run to resolve the attempt on every attempt, got \
                 {described}"
            );
            let output: Value = serde_json::from_str(&string(&described, "output"))
                .expect("the output is JSON");
            assert_eq!(
                output.get(ATTEMPT_FIELD).and_then(Value::as_u64),
                Some(retries),
                "case {case:?}: expected the succeeding attempt to have read {retries}, got \
                 {output}"
            );

            // Every attempt, not only the one that succeeded: the document each hand-over
            // carried reports the attempt it was built on, counting up from none taken.
            let execution = string(&described, "executionArn");
            let events = history(&chain, case, &execution, json!({})).await;
            assert_eq!(
                attempts_handed_over(case, &events, &form),
                attempts_up_to(retries),
                "case {case:?}: expected one hand-over per attempt, each reading the retries \
                 already taken, got the history {events:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A retried `Parallel` and a retried `Map`, whose work is the definition's own
// ---------------------------------------------------------------------------

/// The states the branch — or the iteration — of a retried concurrent state runs: it
/// REPORTS the attempt it was handed once that attempt has reached `retries`, and fails
/// before then, so what decides each attempt's outcome is the number the context object
/// supplied to the state's own data-flow member.
fn deciding_states(retries: u64) -> Value {
    json!({
        "StartAt": "Check",
        "States": {
            "Check": {
                "Type": "Choice",
                "Choices": [{
                    "Variable": ATTEMPT_FIELD_PATH,
                    "NumericLessThan": retries,
                    "Next": "TooEarly",
                }],
                "Default": "Report",
            },
            "TooEarly": {
                "Type": "Fail",
                "Error": TOO_EARLY,
                "Cause": "the state has not been retried enough times yet",
            },
            "Report": { "Type": "Pass", "End": true },
        },
    })
}

/// One concurrent row: the state kind, the member it builds its work's document with, and
/// the execution input that work runs over.
struct Concurrent {
    case: &'static str,
    kind: &'static str,
    /// The state, over the attempt the row succeeds on.
    state: fn(u64) -> Value,
    input: &'static str,
}

/// A `Parallel` state, whose `Parameters` is "the effective input each branch receives".
fn parallel_state(retries: u64) -> Value {
    json!({
        "Type": "Parallel",
        "Parameters": { "retryCount.$": ATTEMPT_PATH },
        "Branches": [deciding_states(retries)],
        "Retry": retry_of("States.ALL"),
        "OutputPath": "$[0]",
        "End": true,
    })
}

/// A `Map` state, whose `ItemSelector` is the document each iteration runs over.
fn map_state(retries: u64) -> Value {
    json!({
        "Type": "Map",
        "ItemsPath": "$.items",
        "ItemSelector": { "retryCount.$": ATTEMPT_PATH },
        "ItemProcessor": deciding_states(retries),
        "Retry": retry_of("States.ALL"),
        "OutputPath": "$[0]",
        "End": true,
    })
}

fn concurrent_rows() -> Vec<Concurrent> {
    vec![
        Concurrent {
            case: "a Parallel state, whose branch reads the document the attempt built",
            kind: "parallel",
            state: parallel_state,
            input: "{}",
        },
        Concurrent {
            case: "a Map state, whose iteration reads the document the attempt built",
            kind: "map",
            state: map_state,
            input: r#"{"items":[1]}"#,
        },
    ]
}

/// The record of `execution_arn` once it has left `RUNNING`, looked for again until it
/// has or [`DEADLINE`] passes.
async fn settled(emulator: &Emulator, case: &str, execution_arn: &str) -> Value {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = describe_execution(emulator, execution_arn).await;
        if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the execution to settle, got {described:?}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}

/// A retried `Parallel` and a retried `Map` read the attempt they are on in exactly the
/// same way: the document each attempt hands the work it starts carries the number of
/// retries already taken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retried_concurrent_state_is_handed_the_attempt_it_is_on() {
    let emulator = Emulator::new();

    for row in concurrent_rows() {
        for retries in RETRY_COUNTS {
            let case = format!("{} succeeding on retry {retries}", row.case);
            let case = case.as_str();
            let definition = json!({
                "StartAt": STATE_NAME,
                "States": { STATE_NAME: (row.state)(retries) },
            })
            .to_string();
            let machine = create_machine(
                &emulator,
                &format!("retry-count-{}-{retries}", row.kind),
                &definition,
            )
            .await;
            let execution = start_execution(&emulator, &machine, "only", row.input).await;

            let described = settled(&emulator, case, &execution).await;
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some(SUCCEEDED),
                "case {case:?}: expected the run to resolve the attempt on every attempt, got \
                 {described:?}"
            );
            let output: Value = serde_json::from_str(&string(&described, "output"))
                .expect("the output is JSON");
            assert_eq!(
                output.get(ATTEMPT_FIELD).and_then(Value::as_u64),
                Some(retries),
                "case {case:?}: expected the succeeding attempt to have read {retries}, got \
                 {output}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The first attempt of a `Task` state, read by the worker it hands its work to
// ---------------------------------------------------------------------------

/// One activity row: how the state reads the context object, and what the document it
/// hands the worker must then be.
struct Claimed {
    case: &'static str,
    /// The members the state carries beside its `Resource`.
    members: fn() -> Value,
    /// Whether the row hands the worker the WHOLE context object, rather than the one
    /// member alone.
    whole: bool,
}

fn claimed_rows() -> Vec<Claimed> {
    vec![
        Claimed {
            case: "the whole context object, which the worker is handed as the document",
            members: || json!({ "Parameters": { "ctx.$": "$$" } }),
            whole: true,
        },
        Claimed {
            case: "the member alone, as a field of the request the state builds",
            members: || json!({ "Parameters": { "retryCount.$": ATTEMPT_PATH } }),
            whole: false,
        },
        Claimed {
            case: "the member read in JSONata, through that language's own context",
            members: || {
                json!({
                    "QueryLanguage": "JSONata",
                    "Arguments": {
                        "retryCount": "{% $states.context.State.RetryCount %}",
                    },
                })
            },
            whole: false,
        },
    ]
}

/// A `Task` state resolves the attempt it is on on its FIRST attempt too: the kind can be
/// retried, so the member is there, and a state that has been retried no times reads zero
/// — whichever language the definition reads it in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_task_state_resolves_the_attempt_on_its_first_attempt() {
    let emulator = Emulator::new();

    for (index, row) in claimed_rows().into_iter().enumerate() {
        let case = row.case;
        let activity = create_activity(&emulator, &format!("retry-count-{index}")).await;
        let definition = activity_definition(&activity, (row.members)());
        let machine = create_machine(
            &emulator,
            &format!("retry-count-activity-{index}"),
            &definition,
        )
        .await;
        let execution = start_execution(&emulator, &machine, "only", "{}").await;

        // The worker is handed exactly what the state built, so the document it claims is
        // where the resolved member is read back.
        let (token, handed) = claimed(&emulator, case, &activity, Some("worker")).await;
        let attempt = if row.whole {
            let state = handed
                .get("ctx")
                .and_then(|context| context.get("State"))
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected the state's members in {handed}"));
            // The members the service supplies for a state of a kind that can be retried:
            // its name, the instant it was entered, and the attempt it is on.
            let mut members: Vec<&str> = state
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: expected an object, got {state}"))
                .keys()
                .map(String::as_str)
                .collect();
            members.sort_unstable();
            assert_eq!(
                members,
                vec!["EnteredTime", "Name", ATTEMPT_MEMBER],
                "case {case:?}: expected the attempt beside the state's name and entry instant, \
                 got {state}"
            );
            state.get(ATTEMPT_MEMBER).and_then(Value::as_u64)
        } else {
            handed.get(ATTEMPT_FIELD).and_then(Value::as_u64)
        };
        assert_eq!(
            attempt,
            Some(0),
            "case {case:?}: expected a state nothing has retried to read no retries taken, got \
             {handed}"
        );

        // And the execution carries on from the resolved document: the worker answers the
        // task it was handed and the run ends on its own result.
        emulator
            .ok(
                "SendTaskSuccess",
                &json!({ "taskToken": token, "output": handed.to_string() }),
            )
            .await;
        let described = settled(&emulator, case, &execution).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some(SUCCEEDED),
            "case {case:?}: expected the answered task to carry the run on, got {described:?}"
        );
    }
}
