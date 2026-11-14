// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DURABLE-EXECUTION programming model a Python handler is written
//! against: the `aws_durable_execution_sdk_python` module this runtime supplies.
//!
//! The rule under test: a managed `python3.x` runtime puts the durable-execution SDK on the
//! handler's import path, so a handler written the way a durable function is written —
//! decorated `@durable_execution`, waiting with `context.wait(Duration.from_seconds(…))`,
//! running a unit of work through `context.step(…)` — IMPORTS and RUNS, its own return value
//! is what the invocation answers and what the durable execution the invocation started
//! reports as its `Result`, and that execution reaches `SUCCEEDED`. Before this held, every
//! such handler failed at module import (`Runtime.ImportModuleError`) and so every execution
//! of a durable function ended `FAILED` — no durable execution could reach a successful
//! terminal status at all, and nothing watching one (a read, or the status-change event the
//! move publishes) could ever report one.
//!
//! The rows differ along what the rule generalises over: WHICH of the model's operations the
//! handler uses (a wait named in seconds, a wait named in milliseconds, a step that computes
//! the result, a step-marked callable called directly, a handler using none of them) and
//! WHAT the handler reads off the context it is handed (the invocation's own members, which
//! the durable context carries through). Every row asserts both halves: what the invocation
//! answered, and what a read of the execution reports.

use std::io::Write as _;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, router_on, send, zip_file, Answer, ROLE,
};

/// Where `Invoke` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The request header `Invoke$DurableExecutionName` is bound to, and the response header
/// `Invoke$DurableExecutionArn` is bound to.
const NAME_HEADER: &str = "x-amz-durable-execution-name";
const ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The header the request id of an invocation is reported on, which one row's handler reads
/// off the context it was handed.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The `ExecutionStatus` a run that returned leaves the execution in
/// (`smithy_query(query="shape",shape="ExecutionStatus")`).
const SUCCEEDED: &str = "SUCCEEDED";

/// The module every package below holds its handler in.
const HANDLER_MODULE: &str = "lambda_function.py";
/// See [`HANDLER_MODULE`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `DurableConfig$ExecutionTimeout` every function here runs under, in seconds: far
/// longer than any row's wait, so nothing times out.
const EXECUTION_TIMEOUT: u64 = 3_600;

/// The function `Timeout`, in seconds: longer than any row's wait, so no run is cut off.
const FUNCTION_TIMEOUT: u64 = 30;

/// The wait one row asks for in SECONDS and another in MILLISECONDS — the shortest spans
/// that still prove the handler's own wait was taken, so no row's outcome turns on how fast
/// the host runs.
const WAIT_SECONDS: f64 = 0.25;
const WAIT_MILLIS: u64 = 250;

/// A .zip deployment package holding `source` as the handler's module.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file(HANDLER_MODULE, options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// A durable function running `source`, brought to the state an invocation needs.
async fn create_durable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
    source: &str,
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "DurableConfig": { "ExecutionTimeout": EXECUTION_TIMEOUT },
            "Code": { "ZipFile": zip_file(&package(source)) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// `GetDurableExecution` on `arn`. The label is not greedy, so the ARN's slashes are
/// percent-encoded in the path exactly as an SDK encodes them.
async fn get_execution(router: &MultiServiceRouter, arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}",
            arn.replace('/', "%2F")
        ),
        &Value::Null,
    )
    .await
}

/// A handler that waits the span its event names, in SECONDS, and reports it.
const WAIT_IN_SECONDS_HANDLER: &str = r#"
from aws_durable_execution_sdk_python import Duration, durable_execution


@durable_execution
def lambda_handler(event, context):
    context.wait(Duration.from_seconds(event["wait_seconds"]))
    return {"waited_seconds": event["wait_seconds"]}
"#;

/// A handler that waits the span its event names, in MILLISECONDS.
const WAIT_IN_MILLIS_HANDLER: &str = r#"
from aws_durable_execution_sdk_python import Duration, durable_execution


@durable_execution
def lambda_handler(event, context):
    context.wait(Duration.from_millis(event["wait_millis"]))
    return {"waited_millis": event["wait_millis"]}
"#;

/// A handler whose result is computed by a STEP: the unit of work really runs, and what it
/// returned is the execution's result.
const STEP_HANDLER: &str = r#"
from aws_durable_execution_sdk_python import durable_execution, durable_step


@durable_step
def total(values):
    return sum(values)


@durable_execution
def lambda_handler(event, context):
    return {"total": context.step(total, event["values"])}
"#;

/// A handler that calls its step-marked callable DIRECTLY, which runs it the same way.
const DIRECT_STEP_HANDLER: &str = r#"
from aws_durable_execution_sdk_python import durable_execution, durable_step


@durable_step(name="join")
def join(words):
    return " ".join(words)


@durable_execution
def lambda_handler(event, context):
    return {"joined": join(event["words"])}
"#;

/// A handler that reads the INVOCATION's own context members through the durable context it
/// was handed, which it must carry through unchanged.
const CONTEXT_HANDLER: &str = r#"
from aws_durable_execution_sdk_python import durable_execution


@durable_execution
def lambda_handler(event, context):
    return {
        "request_id": context.aws_request_id,
        "function_name": context.function_name,
        "remaining_is_positive": context.get_remaining_time_in_millis() > 0,
    }
"#;

/// A durable handler that uses NO operation of the model beyond the decorator.
const PLAIN_HANDLER: &str = r#"
from aws_durable_execution_sdk_python import durable_execution


@durable_execution
def lambda_handler(event, context):
    return {"echoed": event}
"#;

/// One row: the handler, the event it is invoked with, and the result both the invocation and
/// the execution must report.
struct Case {
    case: &'static str,
    source: &'static str,
    event: fn() -> Value,
    /// The result the handler returns, as a function of the invocation's own request id —
    /// the one row that reads the context cannot state it as a literal.
    expected: fn(&str, &str) -> Value,
}

/// RULE: a handler written against the durable-execution programming model imports and runs,
/// its own value is the answer and the execution's result, and the execution reaches
/// `SUCCEEDED`.
#[tokio::test]
async fn a_durable_handler_written_against_the_sdk_runs_and_succeeds() {
    let rows = [
        Case {
            case: "a wait named in seconds",
            source: WAIT_IN_SECONDS_HANDLER,
            event: || json!({ "wait_seconds": WAIT_SECONDS }),
            expected: |_, _| json!({ "waited_seconds": WAIT_SECONDS }),
        },
        Case {
            case: "a wait named in milliseconds",
            source: WAIT_IN_MILLIS_HANDLER,
            event: || json!({ "wait_millis": WAIT_MILLIS }),
            expected: |_, _| json!({ "waited_millis": WAIT_MILLIS }),
        },
        Case {
            case: "a step that computes the result",
            source: STEP_HANDLER,
            event: || json!({ "values": [2, 3, 4] }),
            expected: |_, _| json!({ "total": 9 }),
        },
        Case {
            case: "a step-marked callable called directly",
            source: DIRECT_STEP_HANDLER,
            event: || json!({ "words": ["durable", "step"] }),
            expected: |_, _| json!({ "joined": "durable step" }),
        },
        Case {
            case: "the invocation's own context, read through the durable one",
            source: CONTEXT_HANDLER,
            event: || json!({}),
            expected: |request_id, function| {
                json!({
                    "request_id": request_id,
                    "function_name": function,
                    "remaining_is_positive": true,
                })
            },
        },
        Case {
            case: "a durable handler using no operation of the model",
            source: PLAIN_HANDLER,
            event: || json!({ "from": "the event" }),
            expected: |_, _| json!({ "echoed": { "from": "the event" } }),
        },
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (index, row) in rows.iter().enumerate() {
        let function = format!("durable-sdk-{index}");
        create_durable_function(&router, &engine, &function, row.source).await;
        let execution = format!("exec-{index}");
        let invoked = send(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function}/invocations"),
            "application/json",
            serde_json::to_vec(&(row.event)()).expect("the event serializes"),
            &[(NAME_HEADER, execution.clone())],
        )
        .await;
        assert_eq!(
            invoked.status,
            200,
            "case {:?}: expected 200 from Invoke, got {} {}",
            row.case,
            invoked.status,
            String::from_utf8_lossy(&invoked.body)
        );
        assert_eq!(
            invoked.header("x-amz-function-error"),
            None,
            "case {:?}: the handler must not fault, got {}",
            row.case,
            String::from_utf8_lossy(&invoked.body)
        );
        let request_id = invoked.header(REQUEST_ID_HEADER).unwrap_or_default();
        let expected = (row.expected)(&request_id, &function);
        assert_eq!(
            invoked.json(),
            expected,
            "case {:?}: expected the handler's own value as the answer, got {}",
            row.case,
            String::from_utf8_lossy(&invoked.body)
        );

        // The execution the invocation started reports the same value, and the terminal
        // status a run that returned leaves it in.
        let arn = invoked.header(ARN_HEADER).unwrap_or_else(|| {
            panic!(
                "case {:?}: the invocation of a durable function must answer an execution ARN",
                row.case
            )
        });
        let read = get_execution(&router, &arn).await;
        assert_eq!(
            read.status,
            200,
            "case {:?}: GetDurableExecution must answer 200, got {} {}",
            row.case,
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let reported = read.json();
        assert_eq!(
            reported["Status"].as_str(),
            Some(SUCCEEDED),
            "case {:?}: expected the execution to have reached {SUCCEEDED}, got {reported}",
            row.case
        );
        let result: Value = reported["Result"]
            .as_str()
            .map(|text| serde_json::from_str(text).unwrap_or(Value::Null))
            .unwrap_or(Value::Null);
        assert_eq!(
            result, expected,
            "case {:?}: expected the execution's result to be the handler's own value, got \
             {reported}",
            row.case
        );
    }
}
