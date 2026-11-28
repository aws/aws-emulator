// SPDX-License-Identifier: CC0-1.0

//! The STATE a durable execution's replay resumes from: `GetDurableExecutionState`.
//!
//! RULE: the state is the execution's own operations — "an array of operations that represent
//! the current state of the durable execution[,] ordered by their start sequence number in
//! ascending order" — folded from the history the run really wrote
//! (`crate::durable_execution_checkpoints` pins that history): one `Operation` per operation
//! the run took, carrying its type, its name, the status the last action on it left it in, the
//! instant it started and, once it ended, the instant it ended at and the result or error it
//! came to. And the read is consistent with ONE checkpoint state: `CheckpointToken` "identifies
//! the current state of the execution […] and ensures that state retrieval is consistent with
//! the current execution context", so a token that is not the one currently issued for the
//! execution identifies no state to report and is refused `InvalidParameterValueException`
//! with the sentence the same token rule is refused with on `CheckpointDurableExecution` —
//! which is every token for an execution whose history is closed, because such an execution has
//! no current checkpoint state at all.
//!
//! The rows differ along what the rule generalises over: which operations the run took and what
//! action was last taken on each (a step that succeeded, a step whose attempt failed with
//! another to come, a wait still running, a context that completed), how many operations one
//! page reports, and — for the refusal — WHICH non-current token the read carries (one nothing
//! minted, one that is current for another execution of the same function, one this execution's
//! own checkpoint consumed, and any token at all once the execution is terminal).
//!
//! Nothing here invokes a handler: the executions are started and checkpointed through the same
//! state the `Invoke` and `CheckpointDurableExecution` handlers write, which is what makes the
//! operations read back the ones a run really recorded.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::durable_execution_checkpoints::{checkpoint, current_token, start_execution};
use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, package_bytes, router_on, zip_file, Answer,
    ROLE,
};

/// The `DurableConfig$ExecutionTimeout` every function here runs under, in seconds: far longer
/// than this suite takes, so nothing times out while it reads.
const EXECUTION_TIMEOUT: u64 = 3_600;

/// The function `Timeout`, in seconds.
const FUNCTION_TIMEOUT: u64 = 30;

/// The wire error code a refused read carries, and the sentence a non-current token is refused
/// with (`crate::state::INVALID_CHECKPOINT_TOKEN_MESSAGE`).
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValueException";
const INVALID_CHECKPOINT_TOKEN: &str = "Invalid checkpoint token";

/// The `OperationType` and `OperationStatus` values asserted below, from the model's own enums
/// (`smithy_query(query="shape", shape="OperationType,OperationStatus")`).
const TYPE_EXECUTION: &str = "EXECUTION";
const TYPE_STEP: &str = "STEP";
const TYPE_WAIT: &str = "WAIT";
const TYPE_CONTEXT: &str = "CONTEXT";
const STATUS_STARTED: &str = "STARTED";
const STATUS_PENDING: &str = "PENDING";
const STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// A durable function whose executions this suite starts through the state.
async fn create_durable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    let created = create_function(
        router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": FUNCTION_TIMEOUT,
            "DurableConfig": { "ExecutionTimeout": EXECUTION_TIMEOUT },
            "Code": { "ZipFile": zip_file(&package_bytes(64)) },
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

/// `GetDurableExecutionState` on `arn`, carrying `token` and whatever `query` adds. The label
/// is not greedy, so the ARN's slashes are percent-encoded in the path exactly as an SDK
/// encodes them.
async fn state_with(
    router: &MultiServiceRouter,
    arn: &str,
    token: &str,
    query: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/2025-12-01/durable-executions/{}/state?CheckpointToken={}{query}",
            arn.replace('/', "%2F"),
            token.replace('+', "%2B").replace('/', "%2F").replace('=', "%3D")
        ),
        &Value::Null,
    )
    .await
}

/// `GetDurableExecutionState` on `arn`, one page of the documented default size.
async fn state(router: &MultiServiceRouter, arn: &str, token: &str) -> Answer {
    state_with(router, arn, token, "").await
}

/// The operations an answer reported, in the order it reported them.
fn operations(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Operations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id`, `Type` and `Status` of every operation an answer reported, in order.
fn reported(answer: &Answer) -> Vec<(String, String, String)> {
    operations(answer)
        .iter()
        .map(|operation| {
            let member = |name: &str| {
                operation
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            (member("Id"), member("Type"), member("Status"))
        })
        .collect()
}

/// The refusal sentence an answer carries, under either spelling the model gives the member.
fn message(answer: &Answer) -> String {
    let body = answer.json();
    for name in ["message", "Message"] {
        if let Some(stated) = body.get(name).and_then(Value::as_str) {
            return stated.to_string();
        }
    }
    String::new()
}

/// A well-formed `CheckpointToken` (`^[A-Za-z0-9+/]+={0,2}$`) that is no token this emulator
/// ever minted, which is how a caller sends one it was never handed.
const FORGED_TOKEN: &str = "Zm9yZ2VkLXRva2Vu";

/// RULE: the state reports one operation per operation the run recorded, ordered by where each
/// started, with the status the last action on it left it in and the details of its own type.
#[tokio::test]
async fn the_state_reports_the_operations_the_run_recorded() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "durable-state-operations";
    create_durable_function(&router, &engine, function).await;

    let name = "replayed";
    let arn = start_execution(&engine, function, name, br#"{"n":3}"#);
    let carried =
        current_token(&engine, function, name).expect("a running execution holds a token");

    // The operations the run took, as the runtime's own SDK checkpoints them: a step that
    // succeeded, a step whose first attempt failed with another to come, a wait that is still
    // running, and a context that completed.
    let applied = checkpoint(
        &router,
        &arn,
        &json!({
            "CheckpointToken": carried,
            "Updates": [
                { "Id": "step-1", "Type": "STEP", "Action": "START", "Name": "charge" },
                { "Id": "step-1", "Type": "STEP", "Action": "SUCCEED",
                  "Payload": "{\"ok\":true}" },
                { "Id": "step-2", "Type": "STEP", "Action": "START", "Name": "settle" },
                { "Id": "step-2", "Type": "STEP", "Action": "FAIL",
                  "StepOptions": { "NextAttemptDelaySeconds": 5 },
                  "Error": { "ErrorType": "TimeoutError", "ErrorMessage": "not yet" } },
                { "Id": "wait-1", "Type": "WAIT", "Action": "START",
                  "WaitOptions": { "WaitSeconds": 30 } },
                { "Id": "ctx-1", "Type": "CONTEXT", "Action": "START", "Name": "branch" },
                { "Id": "ctx-1", "Type": "CONTEXT", "Action": "SUCCEED",
                  "Payload": "{\"done\":1}" },
            ],
        }),
    )
    .await;
    assert_eq!(
        applied.status,
        200,
        "the checkpoint of the run's operations must be applied, got {} {}",
        applied.status,
        String::from_utf8_lossy(&applied.body)
    );

    let current =
        current_token(&engine, function, name).expect("the applied checkpoint rotated the token");
    let read = state(&router, &arn, &current).await;
    assert_eq!(
        read.status,
        200,
        "the state of a running execution is answered 200, got {} {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );

    // The EXECUTION itself is the first operation, and every operation the run took follows in
    // the order it started.
    let held = reported(&read);
    let kinds: Vec<(&str, &str)> = held
        .iter()
        .map(|(_, kind, status)| (kind.as_str(), status.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (TYPE_EXECUTION, STATUS_STARTED),
            (TYPE_STEP, STATUS_SUCCEEDED),
            // A failed attempt with another to come leaves the step waiting for it.
            (TYPE_STEP, STATUS_PENDING),
            (TYPE_WAIT, STATUS_STARTED),
            (TYPE_CONTEXT, STATUS_SUCCEEDED),
        ],
        "expected one operation per operation the run recorded, got {:?} out of {}",
        held,
        String::from_utf8_lossy(&read.body)
    );
    let ids: Vec<&str> = held.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(
        &ids[1..],
        &["step-1", "step-2", "wait-1", "ctx-1"],
        "expected every operation to be named by the id its checkpoints carried, got {ids:?}"
    );
    assert!(
        !ids[0].is_empty(),
        "expected the execution's own operation to carry an id, got {held:?}"
    );

    let listed = operations(&read);
    // The EXECUTION carries the payload it was started from, and every operation carries the
    // instant it started.
    assert_eq!(
        listed[0].pointer("/ExecutionDetails/InputPayload"),
        Some(&json!(r#"{"n":3}"#)),
        "expected the execution to report the payload it started from, got {}",
        listed[0]
    );
    for operation in &listed {
        assert!(
            operation.get("StartTimestamp").is_some(),
            "expected every operation to report when it started, got {operation}"
        );
    }
    // The step that SUCCEEDED carries its result, its attempt and the instant it ended; the one
    // waiting for another attempt carries that attempt's instant and the error that failed it.
    assert_eq!(
        listed[1].pointer("/StepDetails/Result"),
        Some(&json!(r#"{"ok":true}"#)),
        "expected the succeeded step to report its result, got {}",
        listed[1]
    );
    assert_eq!(
        listed[1].pointer("/StepDetails/Attempt"),
        Some(&json!(1)),
        "expected the succeeded step to report the attempt it succeeded on, got {}",
        listed[1]
    );
    assert!(
        listed[1].get("EndTimestamp").is_some(),
        "expected the succeeded step to report when it ended, got {}",
        listed[1]
    );
    assert!(
        listed[2]
            .pointer("/StepDetails/NextAttemptTimestamp")
            .is_some(),
        "expected the step waiting for a retry to report when that attempt is due, got {}",
        listed[2]
    );
    assert_eq!(
        listed[2].pointer("/StepDetails/Error/ErrorType"),
        Some(&json!("TimeoutError")),
        "expected the failed attempt to report the error it failed with, got {}",
        listed[2]
    );
    assert!(
        listed[2].get("EndTimestamp").is_none(),
        "expected a step still waiting for an attempt to have no end instant, got {}",
        listed[2]
    );
    // The WAIT that is still running reports when it is scheduled to end and no end instant.
    assert!(
        listed[3]
            .pointer("/WaitDetails/ScheduledEndTimestamp")
            .is_some(),
        "expected the running wait to report its scheduled end, got {}",
        listed[3]
    );
    // The CONTEXT that completed carries its result, and its children need no replay.
    assert_eq!(
        listed[4].pointer("/ContextDetails/Result"),
        Some(&json!(r#"{"done":1}"#)),
        "expected the completed context to report its result, got {}",
        listed[4]
    );
    assert_eq!(
        listed[4].pointer("/ContextDetails/ReplayChildren"),
        Some(&json!(false)),
        "expected a completed context to need no replay of its children, got {}",
        listed[4]
    );

    // One page per `MaxItems`, continued from the marker the page before it reported.
    let first = state_with(&router, &arn, &current, "&MaxItems=2").await;
    let page = reported(&first);
    assert_eq!(
        page.len(),
        2,
        "expected a page of the size the request asked for, got {page:?}"
    );
    let marker = first
        .json()
        .get("NextMarker")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            panic!(
                "expected a truncated page to report where to continue from, got {}",
                String::from_utf8_lossy(&first.body)
            )
        });
    let next = state_with(
        &router,
        &arn,
        &current,
        &format!("&MaxItems=2&Marker={marker}"),
    )
    .await;
    let continued = reported(&next);
    assert_eq!(
        continued.first().map(|(id, _, _)| id.as_str()),
        Some(held[2].0.as_str()),
        "expected the next page to continue at the operation after the marker, got {continued:?}"
    );
}

/// RULE: a read carrying a token that is not the execution's current one is refused
/// `InvalidParameterValueException` / "Invalid checkpoint token" — including every token once
/// the execution's history is closed.
#[tokio::test]
async fn a_state_read_carrying_a_non_current_token_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "durable-state-tokens";
    create_durable_function(&router, &engine, function).await;

    // The execution whose own token is never the one under test, so one row can carry a token
    // that is current somewhere else.
    let other = "other-execution";
    let _other_arn = start_execution(&engine, function, other, b"{}");
    let other_token =
        current_token(&engine, function, other).expect("a running execution holds a token");

    let name = "refused-reads";
    let arn = start_execution(&engine, function, name, b"{}");
    let minted = current_token(&engine, function, name).expect("a running execution holds a token");
    // One applied checkpoint, so the token above is one this execution has CONSUMED.
    let applied = checkpoint(
        &router,
        &arn,
        &json!({ "CheckpointToken": minted, "Updates": [] }),
    )
    .await;
    assert_eq!(
        applied.status,
        200,
        "the checkpoint that consumes the token must be applied, got {} {}",
        applied.status,
        String::from_utf8_lossy(&applied.body)
    );

    // (case, the token the read carries)
    let rows: Vec<(&str, String)> = vec![
        ("a well-formed token nothing minted", FORGED_TOKEN.to_string()),
        (
            "the token that is current for another execution of the same function",
            other_token,
        ),
        ("the token this execution's own checkpoint consumed", minted),
    ];
    for (case, token) in &rows {
        let refused = state(&router, &arn, token).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected HTTP 400, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            INVALID_PARAMETER_VALUE,
            "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {}",
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            message(&refused),
            INVALID_CHECKPOINT_TOKEN,
            "case {case:?}: expected {INVALID_CHECKPOINT_TOKEN:?}, got {}",
            String::from_utf8_lossy(&refused.body)
        );
    }

    // The token that IS current is served, which is what makes the rows above a refusal of the
    // token and not of the read.
    let current = current_token(&engine, function, name)
        .expect("the applied checkpoint rotated the token");
    let served = state(&router, &arn, &current).await;
    assert_eq!(
        served.status,
        200,
        "the current token is served, got {} {}",
        served.status,
        String::from_utf8_lossy(&served.body)
    );

    // And once the history is CLOSED there is no current checkpoint state at all, so the token
    // that was current a moment ago is refused like every other.
    let closed = checkpoint(
        &router,
        &arn,
        &json!({
            "CheckpointToken": current,
            "Updates": [
                { "Id": "execution", "Type": "EXECUTION", "Action": "SUCCEED", "Payload": "{}" },
            ],
        }),
    )
    .await;
    assert_eq!(
        closed.status,
        200,
        "the checkpoint that closes the history must be applied, got {} {}",
        closed.status,
        String::from_utf8_lossy(&closed.body)
    );
    for (case, token) in [
        ("the token the closing checkpoint carried", current.as_str()),
        ("a well-formed token nothing minted", FORGED_TOKEN),
    ] {
        let refused = state(&router, &arn, token).await;
        assert_eq!(
            (refused.status, refused.error_code(), message(&refused)),
            (
                400,
                INVALID_PARAMETER_VALUE.to_string(),
                INVALID_CHECKPOINT_TOKEN.to_string()
            ),
            "case {case:?}: a terminal execution has no current checkpoint state, got {} {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
    }
}
