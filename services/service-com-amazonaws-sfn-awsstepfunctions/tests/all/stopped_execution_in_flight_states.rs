// SPDX-License-Identifier: CC0-1.0

//! What a `StopExecution` catches IN FLIGHT, read back where a client reads it: the
//! execution's own history (`GetExecutionHistory`) and the X-Ray segment tree of the run
//! (`BatchGetTraces`), with Step Functions, AWS Lambda and AWS X-Ray on ONE engine.
//!
//! The RULE under test: **a run that has left the state it ends at has finished, so a
//! service that still owes an execution's outcome reports that state as still running;
//! and the history of a stopped execution is TRUNCATED at every state the stop caught
//! still running — each is entered and never left, no record of its own is written for
//! it, and the execution's own `ExecutionAborted` is the only abort the history holds —
//! while the trace of the run marks each of those states' subsegments `error` with a
//! `Canceled` cause and the run's own segment reports a fault.**
//!
//! The rows differ along what the rule generalises over: the KIND of state the stop catches
//! — each of the kinds `HistoryEventType` models an aborted event for — how that state came
//! to be in flight (a `Wait` the clock has not reached, a `Task` waiting for a worker, a
//! state inside the concurrent work of another, a `Task` whose call has been made while the
//! execution's outcome is still owed), and how deeply that state is nested.
//!
//! Nothing here waits a fixed span: a wait is for the CONDITION — the run's own segment
//! being retrievable — and the deadline is only a backstop against a publication that never
//! happens.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::lambda_task_control_flow::{package, parsed};
use crate::state_machine_lifecycle::{string, REGION, ROLE_ARN};

/// The `origin` the segment of a Step Functions execution carries, spelled out here rather
/// than imported from the implementation, so a wrong constant in the service cannot make a
/// test pass.
const STATE_MACHINE_ORIGIN: &str = "AWS::StepFunctions::StateMachine";

/// What the `cause` of a subsegment covering work a stop prevented states.
const CANCELED_CAUSE: &str = "Canceled";

/// The event an execution that was stopped ends on.
const EXECUTION_ABORTED: &str = "ExecutionAborted";

/// The suffix of every event type a state records when the work around it was stopped
/// while that state was still running.
const STATE_ABORTED_SUFFIX: &str = "StateAborted";

/// The statuses an execution reports before and after a stop.
const RUNNING: &str = "RUNNING";
const ABORTED: &str = "ABORTED";

/// A `Wait` far longer than this suite runs, so a row's state really is in flight for the
/// whole of it rather than for as long as the host took to answer.
const HELD_SECONDS: u64 = 900;

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`), and the `State` a
/// function reports once it can be invoked.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";
const FUNCTION_ACTIVE: &str = "Active";

/// The runtime, handler and role of the function the invoking row calls. No IAM is mounted
/// here, so nothing is known about the role and Lambda serves the create.
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";
const LAMBDA_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/LambdaExecutionRole";

/// The handler the invoking row calls: it answers at once, so the work of the task is over
/// while the execution's own outcome is still owed.
const HANDLER_SOURCE: &str = r#"
def handler(event, context):
    return {"ok": True}
"#;

/// How long a wait for a condition may take, and how often it looks. Generous: the deadline
/// is a backstop against something that never happens, not a measurement.
const DEADLINE: Duration = Duration::from_secs(90);
const LOOK: Duration = Duration::from_millis(50);

/// How many times the invoking row starts and stops an execution before giving up.
///
/// That row needs its stop to land while the execution's outcome is still owed, which is a
/// window of the SERVICE's own; a row that lost the race starts another execution rather
/// than asserting anything about how fast the host answered.
const STOP_ATTEMPTS: usize = 8;

/// One emulator: Step Functions, AWS Lambda and AWS X-Ray mounted on ONE engine, the shape
/// `core/router/src/lib.rs` mounts them in — so a task's invocation is served by the very
/// function a row created and the run's segments land in the store `BatchGetTraces` answers
/// from.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what
    /// routes the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        )
    }

    /// One Step Functions call over its `awsJson1_0` wire, requiring a 200.
    async fn sfn(&self, operation: &str, body: &Value) -> Value {
        let host = format!("states.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}/"))
            .header("host", &host)
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
            .header("authorization", Self::authorization("states"))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = parsed(response.into_body());
        assert_eq!(
            status, 200,
            "{operation}: expected 200, got {status} with body {answered}"
        );
        answered
    }

    /// One X-Ray call over its `restJson1` wire, requiring a 200.
    async fn xray(&self, path: &str, body: &Value) -> Value {
        let host = format!("xray.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", &host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("xray"))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = parsed(response.into_body());
        assert_eq!(
            status, 200,
            "POST {path}: expected 200, got {status} with body {answered}"
        );
        answered
    }

    /// One `restJson1` call to Lambda, as its status and parsed body.
    async fn lambda(&self, method: &str, uri: &str, body: &Value) -> (u16, Value) {
        let payload = match body.is_null() {
            true => Vec::new(),
            false => body.to_string().into_bytes(),
        };
        let host = format!("lambda.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", &host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("lambda"))
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(response.into_body()))
    }

    /// The function the invoking row calls, waited out to [`FUNCTION_ACTIVE`] so an
    /// invocation of it is one Lambda admits; answers its ARN.
    async fn given_function(&self, name: &str) -> String {
        let (status, created) = self
            .lambda(
                "POST",
                FUNCTIONS_URI,
                &json!({
                    "FunctionName": name,
                    "Role": LAMBDA_ROLE_ARN,
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Code": { "ZipFile": package(HANDLER_SOURCE) },
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateFunction must answer 201, got {status} with {created}"
        );
        let arn = string(&created, "FunctionArn");
        let deadline = Instant::now() + DEADLINE;
        loop {
            let (status, configuration) = self
                .lambda(
                    "GET",
                    &format!("{FUNCTIONS_URI}/{name}/configuration"),
                    &Value::Null,
                )
                .await;
            assert_eq!(
                status, 200,
                "GetFunctionConfiguration must resolve the created function, got {status} with \
                 {configuration}"
            );
            if configuration.get("State").and_then(Value::as_str) == Some(FUNCTION_ACTIVE) {
                return arn;
            }
            assert!(
                Instant::now() < deadline,
                "the function must reach {FUNCTION_ACTIVE}, last saw {configuration}"
            );
            ::tokio::time::sleep(LOOK).await;
        }
    }

    /// The activity the polling row's `Task` state waits at; answers its ARN.
    async fn given_activity(&self, name: &str) -> String {
        let created = self.sfn("CreateActivity", &json!({ "name": name })).await;
        string(&created, "activityArn")
    }

    /// One TRACING state machine running `definition`; answers its ARN.
    async fn given_state_machine(&self, name: &str, definition: &Value) -> String {
        let created = self
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": ROLE_ARN,
                    "tracingConfiguration": { "enabled": true },
                }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Start one execution of `machine` on `input`, under the chain `root`; answers its ARN.
    async fn started(&self, machine: &str, name: &str, input: &Value, root: &str) -> String {
        let started = self
            .sfn(
                "StartExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": name,
                    "input": input.to_string(),
                    "traceHeader": format!("Root={root};Sampled=1"),
                }),
            )
            .await;
        string(&started, "executionArn")
    }

    /// The status `DescribeExecution` reports for `execution_arn`.
    async fn status_of(&self, execution_arn: &str) -> String {
        let described = self
            .sfn("DescribeExecution", &json!({ "executionArn": execution_arn }))
            .await;
        string(&described, "status")
    }

    /// Every event of `execution_arn`'s history, in the order the service reports them.
    async fn history(&self, execution_arn: &str) -> Vec<Value> {
        let answered = self
            .sfn(
                "GetExecutionHistory",
                &json!({ "executionArn": execution_arn }),
            )
            .await;
        answered
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("expected an events list, got {answered}"))
            .clone()
    }

    /// Wait until X-Ray holds the SEGMENT of the run the chain `root` was started under —
    /// the one segment of that trace whose `origin` names a state machine — and answer its
    /// document.
    async fn awaited_segment(&self, case: &str, root: &str) -> Value {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let answered = self.xray("/Traces", &json!({ "TraceIds": [root] })).await;
            let documents = segment_documents(&answered);
            if let Some(document) = documents.iter().find(|document| {
                document.get("origin").and_then(Value::as_str) == Some(STATE_MACHINE_ORIGIN)
            }) {
                return document.clone();
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: X-Ray never held a segment of the chain {root:?}, last saw \
                 {answered}"
            );
            ::tokio::time::sleep(LOOK).await;
        }
    }
}

/// Every segment document a `BatchGetTraces` answer carries, parsed.
fn segment_documents(answered: &Value) -> Vec<Value> {
    answered
        .get("Traces")
        .and_then(Value::as_array)
        .map(|traces| {
            traces
                .iter()
                .flat_map(|trace| {
                    trace
                        .get("Segments")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|segment| {
                    segment
                        .get("Document")
                        .and_then(Value::as_str)
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A chain of X-Ray's own form (`1-<8 hex>-<24 hex>`), one per run, so a run's own trace is
/// asked for by the id its start supplied rather than looked for among the others.
fn chain(run: usize) -> String {
    format!("1-5759e988-bd862e3fe1be46a99427{run:04x}")
}

/// The entity at `path` below `document`: each step naming a subsegment of the one before
/// it.
fn entity_at<'a>(document: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut entity = document;
    for step in path {
        entity = document_children(entity)
            .iter()
            .find(|child| child.get("name").and_then(Value::as_str) == Some(*step))?;
    }
    Some(entity)
}

/// The subsegments `entity` carries, in the order it carries them.
fn document_children(entity: &Value) -> &[Value] {
    entity
        .get("subsegments")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// The message the `cause` of `entity` states, and the message of the one exception beside
/// it: a reader attributes the stop by either.
fn cause_messages(entity: &Value) -> (Option<String>, Option<String>) {
    let cause = entity.get("cause");
    let stated = cause
        .and_then(|cause| cause.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let raised = cause
        .and_then(|cause| cause.get("exceptions"))
        .and_then(Value::as_array)
        .and_then(|exceptions| exceptions.first())
        .and_then(|exception| exception.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    (stated, raised)
}

/// The type of every event of `events` that reports a state the stop caught in flight, in
/// the order the history reports them.
///
/// A stopped execution's history holds NONE: `HistoryEventType` models a
/// `<Type>StateAborted` value per kind of state that can still be running
/// (`smithy_query(query="shape",shape="HistoryEventType")`), and the walk writes one for
/// each state another unit's FAILURE stopped, but a stop truncates the history at the
/// states it caught instead — so this is what such a history must not carry.
fn aborted_events(events: &[Value]) -> Vec<String> {
    types(events)
        .into_iter()
        .filter(|event_type| event_type.ends_with(STATE_ABORTED_SUFFIX))
        .collect()
}

/// The name of every state `events` reports as ENTERED, and of every one it reports as
/// LEFT: `HistoryEvent` carries a detail structure for each
/// (`smithy_query(query="shape",shape="HistoryEvent")`), and the state's own name is the
/// `name` member of it.
fn entered_and_left(events: &[Value]) -> (Vec<String>, Vec<String>) {
    let named = |member: &str| -> Vec<String> {
        events
            .iter()
            .filter_map(|event| event.get(member))
            .filter_map(|details| details.get("name").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    };
    (
        named("stateEnteredEventDetails"),
        named("stateExitedEventDetails"),
    )
}

/// The `type` of each event, in the order the history reports them.
fn types(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// One row: a definition whose run is in flight at a state of a given kind, and what the
/// stop of it must report.
struct Row {
    case: &'static str,
    definition: Value,
    input: Value,
    /// The states the stop catches still running, innermost first: each is entered and
    /// never left, and records nothing of its own.
    unfinished: Vec<&'static str>,
    /// Where in the segment tree each of those states is reported, in the same order.
    canceled: Vec<Vec<&'static str>>,
}

/// A definition whose one `Wait` holds for far longer than this suite runs.
fn held(name: &str) -> Value {
    json!({
        "StartAt": name,
        "States": { name: { "Type": "Wait", "Seconds": HELD_SECONDS, "End": true } },
    })
}

/// Every state a stop catches in flight is left unfinished: the history is truncated at it
/// — it is entered, never left, and records nothing of its own — and the trace of the run
/// marks its subsegment.
///
/// The kinds are the catalog `HistoryEventType` declares an aborted event for —
/// `WaitStateAborted`, `TaskStateAborted`, `MapStateAborted` and `ParallelStateAborted`
/// (`smithy_query(query="shape",shape="HistoryEventType")`) — one row per kind, each with
/// that kind of state really in flight, and none of those events is what a STOP writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_leaves_every_state_it_catches_in_flight_unfinished() {
    let emulator = Emulator::new();
    let activity = emulator.given_activity("worked-on").await;
    let rows = [
        Row {
            case: "a Wait the clock has not reached",
            definition: held("Hold"),
            input: json!({}),
            unfinished: vec!["Hold"],
            canceled: vec![vec!["Hold"]],
        },
        Row {
            case: "a Task waiting for an activity worker",
            definition: json!({
                "StartAt": "Work",
                "States": {
                    "Work": { "Type": "Task", "Resource": activity, "End": true },
                },
            }),
            input: json!({}),
            unfinished: vec!["Work"],
            canceled: vec![vec!["Work"]],
        },
        Row {
            case: "a Wait inside a Parallel branch, and the Parallel around it",
            definition: json!({
                "StartAt": "Fan",
                "States": {
                    "Fan": {
                        "Type": "Parallel",
                        "Branches": [held("Hold")],
                        "End": true,
                    },
                },
            }),
            input: json!({}),
            unfinished: vec!["Hold", "Fan"],
            canceled: vec![vec!["Fan", "Branch 0", "Hold"], vec!["Fan"]],
        },
        Row {
            case: "a Wait inside a Map iteration, and the Map around it",
            definition: json!({
                "StartAt": "Each",
                "States": {
                    "Each": {
                        "Type": "Map",
                        "ItemsPath": "$.values",
                        "MaxConcurrency": 1,
                        "ItemProcessor": held("Hold"),
                        "End": true,
                    },
                },
            }),
            input: json!({ "values": [1, 2] }),
            unfinished: vec!["Hold", "Each"],
            canceled: vec![vec!["Each", "Iteration 0", "Hold"], vec!["Each"]],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine = emulator
            .given_state_machine(&format!("caught-{index}"), &row.definition)
            .await;
        let root = chain(index);
        let execution = emulator.started(&machine, "only", &row.input, &root).await;
        // The run really is in flight: its own work is owed something nothing here
        // answers, so the execution is running until this stop and not because of how
        // fast the host got here.
        let running = emulator.status_of(&execution).await;
        assert_eq!(
            running, RUNNING,
            "case {case:?}: expected the run to be {RUNNING} before it is stopped, got {running}"
        );
        emulator
            .sfn("StopExecution", &json!({ "executionArn": execution }))
            .await;
        let stopped = emulator.status_of(&execution).await;
        assert_eq!(
            stopped, ABORTED,
            "case {case:?}: expected the stopped run to be {ABORTED}, got {stopped}"
        );

        // The history is truncated at every state the stop caught: each was entered and
        // is never left, nothing of its own is recorded for it, and the end of the
        // execution itself is the one abort the history holds.
        let events = emulator.history(&execution).await;
        assert!(
            aborted_events(&events).is_empty(),
            "case {case:?}: expected the stop to record no per-state abort, got {events:?}"
        );
        let (entered, left) = entered_and_left(&events);
        for state in &row.unfinished {
            assert!(
                entered.contains(&(*state).to_string()),
                "case {case:?}: expected {state:?} to have been entered, got {entered:?}"
            );
            assert!(
                !left.contains(&(*state).to_string()),
                "case {case:?}: expected the stopped run never to report {state:?} as left, got \
                 {left:?}"
            );
        }
        assert_eq!(
            types(&events).last().map(String::as_str),
            Some(EXECUTION_ABORTED),
            "case {case:?}: expected the history to end on {EXECUTION_ABORTED}, got {events:?}"
        );

        // And the trace of the run reports the same thing: the run itself faulted, and
        // every state the stop caught marked as work that was canceled.
        let document = emulator.awaited_segment(case, &root).await;
        assert_eq!(
            document.get("fault").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the stopped run's own segment to report a fault, got \
             {document}"
        );
        for path in &row.canceled {
            let entity = entity_at(&document, path).unwrap_or_else(|| {
                panic!("case {case:?}: expected a subsegment at {path:?}, got {document}")
            });
            assert_eq!(
                entity.get("error").and_then(Value::as_bool),
                Some(true),
                "case {case:?}: expected {path:?} to be marked in error, got {entity}"
            );
            assert_eq!(
                cause_messages(entity),
                (
                    Some(CANCELED_CAUSE.to_string()),
                    Some(CANCELED_CAUSE.to_string())
                ),
                "case {case:?}: expected {path:?} to state the cause {CANCELED_CAUSE:?}, got \
                 {entity}"
            );
        }
    }
}

/// A `Task` whose work was a CALL to another service is in flight for as long as the
/// execution's own outcome is owed: a stop that lands then catches that task, so the run
/// never reports it as left, the history holds no record of its own for it, and the trace
/// marks it.
///
/// This is the rule over the state kind whose work leaves this service: the call is made
/// and answered inside the start, and the state the run ends at is reported as left only
/// when the outcome it produces is reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_catches_the_task_whose_outcome_the_run_still_owes() {
    let case = "a Task invoking a function, stopped while the outcome is owed";
    let emulator = Emulator::new();
    let function = emulator.given_function("stopped-task").await;
    let machine = emulator
        .given_state_machine(
            "owed",
            &json!({
                "StartAt": "Call",
                "States": {
                    "Call": {
                        "Type": "Task",
                        "Resource": "arn:aws:states:::lambda:invoke",
                        "Parameters": { "FunctionName": function, "Payload.$": "$" },
                        "End": true,
                    },
                },
            }),
        )
        .await;

    // The stop must land while the outcome is still owed, which is a window of the
    // service's own: a run whose outcome had already come due is not one a stop reaches,
    // so another execution is started rather than anything asserted about the host.
    let mut caught: Option<(String, String)> = None;
    for attempt in 0..STOP_ATTEMPTS {
        let root = chain(0x100 + attempt);
        let execution = emulator
            .started(&machine, &format!("stopped-{attempt}"), &json!({}), &root)
            .await;
        emulator
            .sfn("StopExecution", &json!({ "executionArn": execution }))
            .await;
        if emulator.status_of(&execution).await == ABORTED {
            caught = Some((execution, root));
            break;
        }
    }
    let (execution, root) = caught.unwrap_or_else(|| {
        panic!(
            "case {case:?}: no stop of {STOP_ATTEMPTS} landed while the run's outcome was owed"
        )
    });

    // The run never left the state it ends at: the exit of that state is reported with the
    // outcome, and this run reached none.
    let events = emulator.history(&execution).await;
    let reported = types(&events);
    assert!(
        reported.iter().any(|event| event == "TaskStateEntered"),
        "case {case:?}: expected the task to have been entered, got {events:?}"
    );
    assert!(
        !reported.iter().any(|event| event == "TaskStateExited"),
        "case {case:?}: expected the stopped run never to report the task as left, got {events:?}"
    );
    assert!(
        aborted_events(&events).is_empty(),
        "case {case:?}: expected the stop to record no per-state abort for the task, got \
         {events:?}"
    );
    assert_eq!(
        reported.last().map(String::as_str),
        Some(EXECUTION_ABORTED),
        "case {case:?}: expected the history to end on {EXECUTION_ABORTED}, got {events:?}"
    );

    // And the trace reports it: the run faulted, and the task the stop caught carries the
    // error and the cause of work that was canceled.
    let document = emulator.awaited_segment(case, &root).await;
    assert_eq!(
        document.get("fault").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: expected the stopped run's own segment to report a fault, got {document}"
    );
    let task = entity_at(&document, &["Call"]).unwrap_or_else(|| {
        panic!("case {case:?}: expected a subsegment for the task, got {document}")
    });
    assert_eq!(
        task.get("error").and_then(Value::as_bool),
        Some(true),
        "case {case:?}: expected the task the stop caught to be marked in error, got {task}"
    );
    assert_eq!(
        cause_messages(task),
        (
            Some(CANCELED_CAUSE.to_string()),
            Some(CANCELED_CAUSE.to_string())
        ),
        "case {case:?}: expected the task to state the cause {CANCELED_CAUSE:?}, got {task}"
    );
}
