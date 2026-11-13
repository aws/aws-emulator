// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the RUN of a durable function does to the EXECUTION that
//! invocation started.
//!
//! The rule under test: a durable execution is the handle on one invocation of a durable
//! function, so the status it ends in is the status that invocation's run produced — a
//! handler that returned makes it `SUCCEEDED` carrying the result it returned, a run that
//! failed makes it `FAILED` carrying that failure's error, and the history records the
//! terminal event beside the `ExecutionStarted` it opened with, event ids increasing. An
//! execution already in a terminal status is not re-opened by the run settling afterwards.
//!
//! Every row drives real HTTP requests through the router against a function whose
//! deployment package RUNS, and reads the execution back through `GetDurableExecution` and
//! `GetDurableExecutionHistory`, so a status this emulator moved without a run behind it
//! fails here. The rows differ along what the rule generalises over: how the handler ends
//! (a document, a bare value, no value at all, an exception), whether this emulator can run
//! the package at all, and whether a caller had already ended the execution.
//!
//! Every wait here is for a CONDITION and never for a span of wall clock. What settles an
//! execution is the delivery of the invocation it is the handle on, and the emulator bounds
//! ONE delivery attempt and the wait between two of them and nothing else — an attempt that
//! found no capacity to run the function ran nothing, so the event is attempted again for as
//! long as it is retained, and how many attempts a loaded host costs it is not a row's to
//! bound. So a row waits for that delivery with the progress-aware helper the asynchronous
//! invoke suite publishes ([`settled_newest`]), which fails only on the emulator going quiet
//! about the event, and then looks at `GetDurableExecution` again until it reports what the
//! run produced. Every limit no row is about — the `Timeout` the handler runs under above all
//! — is far above what the work takes, so how fast the host runs decides nothing asserted
//! here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::HOST_WAIT_CEILING;
use service_com_amazonaws_lambda_awsgirapiservice::state::AsyncInvocationOutcome;
use smithy_server_runtime::MultiServiceRouter;

// Waiting for the DELIVERY of an accepted asynchronous invocation is the same wait the
// asynchronous invoke suite makes, so this one makes it with the helper that suite publishes
// rather than with a budget of its own: it looks at the queue entry again until the attempt
// has settled, and fails only when nothing is working on the event any more.
use crate::asynchronous_invoke_run::settled_newest;
use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, send, zip_file,
    Answer, ROLE,
};
use crate::synchronous_invoke_run::package;

/// Where `Invoke` is bound (`POST /2015-03-31/functions/{FunctionName}/invocations`).
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// The response header `Invoke$DurableExecutionArn` is bound to.
const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The request header `Invoke$DurableExecutionName` is bound to.
const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";

/// The handler every package here exposes.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `ExecutionTimeout` the durable functions here run under: far longer than any run in
/// this suite takes, so no row's execution is settled by the clock instead of by its run.
const EXECUTION_TIMEOUT: i64 = 3_600;

/// The model's `ExecutionStatus` values (`smithy_query(query="shape",
/// shape="ExecutionStatus")`).
const STATUS_RUNNING: &str = "RUNNING";
const STATUS_SUCCEEDED: &str = "SUCCEEDED";
const STATUS_FAILED: &str = "FAILED";
const STATUS_STOPPED: &str = "STOPPED";

/// The model's `EventType` an execution's history opens with, and the two a run closes it
/// with.
const EVENT_EXECUTION_STARTED: &str = "ExecutionStarted";
const EVENT_EXECUTION_SUCCEEDED: &str = "ExecutionSucceeded";
const EVENT_EXECUTION_FAILED: &str = "ExecutionFailed";

/// The `Timeout` every function here runs its handler under.
///
/// Generous on purpose, because no row here is about the timeout: a handler stopped at that
/// deadline is reported as a run that ERRORED, so a tight value would turn a loaded host into
/// a failure the handler never raised. Minutes for handlers that return in milliseconds, and
/// well inside the range the model allows the member (`smithy_query(query="shape",
/// shape="Timeout")`: 1 to 5400).
const GENEROUS_TIMEOUT_SECONDS: i64 = 300;

/// How long a row waits for the execution to report what a SETTLED run produced.
///
/// The run itself is not bounded here at all — it is waited for as the condition it is
/// ([`settled_newest`]) — and between the queue entry carrying what the attempt came to and
/// the execution carrying it there is no span this emulator waits out: the same delivery task
/// does the one after the other. So this is only room for the host to get to that task, taken
/// generously from a span the emulator itself allows one run ([`HOST_WAIT_CEILING`]) rather
/// than picked here, and it is a backstop against an emulator that settles the invocation
/// without ever settling the execution.
const HANDOVER_CEILING: Duration = HOST_WAIT_CEILING;

/// How often a row looks again while it waits.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// `Invoke` on `reference`, naming `execution` as the durable execution: an asynchronous
/// invocation, which is the one that starts a durable execution and answers its ARN.
async fn invoke(router: &MultiServiceRouter, reference: &str, execution: &str) -> Answer {
    send(
        router,
        "POST",
        &format!("{INVOCATIONS_URI}/{reference}/invocations"),
        "application/octet-stream",
        b"{\"n\": 21}".to_vec(),
        &[
            ("x-amz-invocation-type", "Event".to_string()),
            (DURABLE_EXECUTION_NAME_HEADER, execution.to_string()),
        ],
    )
    .await
}

/// `GetDurableExecution` on the execution `arn` addresses.
async fn get_execution(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2025-12-01/durable-executions/{}", arn.replace('/', "%2F")),
        &Value::Null,
    )
    .await
}

/// `GetDurableExecutionHistory` on the same handle.
async fn get_history(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}/history",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// `StopDurableExecution` on the same handle, carrying no reason.
async fn stop_execution(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/2025-12-01/durable-executions/{}/stop",
            arn.replace('/', "%2F")
        ),
        &json!({}),
    )
    .await
}

/// Create a durable function whose package is `code`, and bring it to `Active`.
async fn given_durable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    code: &[u8],
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": GENEROUS_TIMEOUT_SECONDS,
            "Code": { "ZipFile": zip_file(code) },
            "DurableConfig": { "ExecutionTimeout": EXECUTION_TIMEOUT },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// Start one execution of `function` and answer the ARN it is addressed by.
async fn started(router: &MultiServiceRouter, function: &str, execution: &str) -> String {
    let accepted = invoke(router, function, execution).await;
    assert_eq!(
        accepted.status,
        202,
        "case {execution}: the invocation must be accepted, got {} {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    accepted
        .header(DURABLE_EXECUTION_ARN_HEADER)
        .unwrap_or_else(|| panic!("case {execution}: the invocation must answer an execution ARN"))
}

/// The execution `arn` addresses, once the RUN of the invocation it is the handle on has been
/// made and the execution has left `RUNNING`.
///
/// Two waits, each for a condition: the delivery of that invocation, waited for however many
/// attempts a loaded host costs it ([`settled_newest`]), and then the execution reporting what
/// the attempt came to, looked for again until it does.
async fn settled(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    function: &str,
    arn: &str,
) -> Value {
    let delivered = settled_newest(engine, case, function).await;
    let deadline = Instant::now() + HANDOVER_CEILING;
    loop {
        let answer = get_execution(router, arn).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: GetDurableExecution must answer 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let execution = answer.json();
        let status = execution
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if status != STATUS_RUNNING {
            return execution;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the run of this execution's invocation came to {:?}, so the \
             execution must report what that run produced, and it is still {status} \
             {HANDOVER_CEILING:?} later: {execution}",
            delivered.outcome
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The `EventType` of every event the history reported, in order.
fn event_types(answer: &Answer) -> Vec<String> {
    answer
        .json()
        .get("Events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    event
                        .get("EventType")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the settlement table: the handler the function runs, and what the execution
/// its invocation started must therefore report.
struct Row {
    /// Names the row.
    case: &'static str,
    /// The handler source the function's package carries.
    source: &'static str,
    /// The `Status` the execution must end in.
    status: &'static str,
    /// The `Result` it must report, parsed, or `None` for an execution that reports none.
    result: Option<Value>,
    /// The `Error.ErrorType` it must report, or `None` for an execution that reports none.
    error_type: Option<&'static str>,
    /// The `EventType` its history must close with.
    terminal: &'static str,
}

/// A durable execution ends in the status its invocation's run produced, carrying what that
/// run produced.
#[tokio::test]
async fn a_durable_execution_ends_in_the_status_its_run_produced() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let rows: Vec<Row> = vec![
        Row {
            case: "a handler that returns a document",
            source: "def lambda_handler(event, context):\n    return {'doubled': event['n'] * 2}\n",
            status: STATUS_SUCCEEDED,
            result: Some(json!({"doubled": 42})),
            error_type: None,
            terminal: EVENT_EXECUTION_SUCCEEDED,
        },
        Row {
            case: "a handler that returns a bare value",
            source: "def lambda_handler(event, context):\n    return event['n']\n",
            status: STATUS_SUCCEEDED,
            result: Some(json!(21)),
            error_type: None,
            terminal: EVENT_EXECUTION_SUCCEEDED,
        },
        Row {
            case: "a handler that returns no value at all",
            source: "def lambda_handler(event, context):\n    return None\n",
            status: STATUS_SUCCEEDED,
            result: Some(Value::Null),
            error_type: None,
            terminal: EVENT_EXECUTION_SUCCEEDED,
        },
        Row {
            case: "a handler that raises",
            source: "def lambda_handler(event, context):\n    raise ValueError('the step failed')\n",
            status: STATUS_FAILED,
            result: None,
            error_type: Some("ValueError"),
            terminal: EVENT_EXECUTION_FAILED,
        },
    ];
    for (index, row) in rows.iter().enumerate() {
        let Row {
            case,
            source,
            status,
            result,
            error_type,
            terminal,
        } = row;
        let function = format!("durable-run-{index}");
        given_durable_function(&router, &engine, &function, &package(source)).await;
        let arn = started(&router, &function, "the-execution").await;
        let execution = settled(&router, &engine, case, &function, &arn).await;

        assert_eq!(
            execution.get("Status").and_then(Value::as_str),
            Some(*status),
            "case {case:?}: expected the status {status}, got {execution}"
        );
        assert!(
            execution.get("EndTimestamp").is_some(),
            "case {case:?}: a settled execution reports when it ended, got {execution}"
        );
        match result {
            Some(expected) => {
                let reported = execution
                    .get("Result")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let parsed: Value = serde_json::from_str(&reported).unwrap_or(Value::Null);
                assert_eq!(
                    &parsed, expected,
                    "case {case:?}: expected the result {expected}, got {reported:?}"
                );
            }
            None => assert!(
                execution.get("Result").is_none(),
                "case {case:?}: a failed execution reports no result, got {execution}"
            ),
        }
        match error_type {
            Some(expected) => assert_eq!(
                execution.pointer("/Error/ErrorType").and_then(Value::as_str),
                Some(*expected),
                "case {case:?}: expected the error type {expected}, got {execution}"
            ),
            None => assert!(
                execution.get("Error").is_none(),
                "case {case:?}: a successful execution reports no error, got {execution}"
            ),
        }

        // The history records what happened: the start it opened with, then the terminal
        // event the run produced, with ids increasing.
        let history = get_history(&router, &arn).await;
        assert_eq!(
            event_types(&history),
            vec![EVENT_EXECUTION_STARTED.to_string(), (*terminal).to_string()],
            "case {case:?}: expected the start and the terminal event, got {}",
            String::from_utf8_lossy(&history.body)
        );
    }
}

/// A run this emulator cannot make at all still ends the execution: it reports the failure
/// with the reason nothing ran, and never leaves the execution RUNNING for ever.
#[tokio::test]
async fn an_execution_whose_package_cannot_be_run_fails_with_the_reason() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // Bytes that are no archive at all: there is nothing in them to run.
    given_durable_function(&router, &engine, "durable-unreadable", &package_bytes(64)).await;
    let arn = started(&router, "durable-unreadable", "unreadable").await;
    let execution = settled(
        &router,
        &engine,
        "a package that is no archive",
        "durable-unreadable",
        &arn,
    )
    .await;

    assert_eq!(
        execution.get("Status").and_then(Value::as_str),
        Some(STATUS_FAILED),
        "case \"a package that is no archive\": expected {STATUS_FAILED}, got {execution}"
    );
    let message = execution
        .pointer("/Error/ErrorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("could not be read"),
        "case \"a package that is no archive\": expected the reason nothing ran, got \
         {message:?}"
    );
    assert!(
        execution.pointer("/Error/ErrorType").is_none(),
        "case \"a package that is no archive\": no function raised, so there is no error type, \
         got {execution}"
    );
}

/// An execution a caller already ENDED is not re-opened by the run settling afterwards.
#[tokio::test]
async fn a_stopped_execution_is_not_re_opened_by_its_run() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    given_durable_function(
        &router,
        &engine,
        "durable-stopped",
        &package("def lambda_handler(event, context):\n    return {'ok': True}\n"),
    )
    .await;
    let arn = started(&router, "durable-stopped", "stopped-before-the-run").await;

    let stopped = stop_execution(&router, &arn).await;
    assert_eq!(
        stopped.status,
        200,
        "StopDurableExecution must answer 200, got {} {}",
        stopped.status,
        String::from_utf8_lossy(&stopped.body)
    );

    // Past the delivery and the run it makes, the stop still stands. The run is waited for as
    // the condition it is — the queue entry carrying what its attempt came to — so the stop is
    // read back after a run that really happened rather than after a span of wall clock.
    let case = "an execution stopped before its run finished";
    let delivered = settled_newest(&engine, case, "durable-stopped").await;
    assert!(
        matches!(
            delivered.outcome,
            AsyncInvocationOutcome::Delivered {
                function_error: None
            }
        ),
        "case {case:?}: the handler of this function must have run and returned, so the stop is \
         read back past the run that could re-open the execution, got {:?}",
        delivered.outcome
    );
    let execution = get_execution(&router, &arn).await.json();
    assert_eq!(
        execution.get("Status").and_then(Value::as_str),
        Some(STATUS_STOPPED),
        "case {case:?}: expected {STATUS_STOPPED}, got {execution}"
    );
    assert!(
        execution.get("Result").is_none(),
        "case {case:?}: a stopped execution reports no result, got {execution}"
    );
}
