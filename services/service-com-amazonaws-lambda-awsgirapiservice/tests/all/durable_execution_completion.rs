// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a DURABLE EXECUTION ends.
//!
//! The rule under test: an execution is the handle a client polls, and it is over when the
//! invocation that drives it is over. `GetDurableExecution` reports `RUNNING` while the
//! function is still running it, and then the terminal status that invocation reached: a
//! handler that returned ends the execution `SUCCEEDED`, with an `EndTimestamp` and an
//! `ExecutionSucceeded` history event carrying what the handler returned; a handler that
//! raised ends it `FAILED`, with an `ExecutionFailed` event and the error the run produced.
//! An execution left `RUNNING` after its invocation has ended is a wait no event would ever
//! end, which is why nothing here is reported until the run that decides it has happened.
//!
//! The rows differ along what the rule generalises over: how the handler ends (a return, a
//! raise), what it returns, and the error it raises.
//!
//! The package writer is the one `synchronous_invoke_run.rs` publishes, so there is one
//! writer of a deployment archive in this target.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    HOST_WAIT_CEILING, PERMIT_WAIT,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ACTIVATION_DELAY, FIRST_ASYNC_RETRY_DELAY, SECOND_ASYNC_RETRY_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{create_function, json_call, router_on, send, Answer, ROLE};
use crate::synchronous_invoke_run::package;

/// Where `Invoke` is bound.
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// The request header `Invoke$DurableExecutionName` is bound to.
const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";

/// The response header `Invoke$DurableExecutionArn` is bound to.
const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// A customer managed key ARN satisfying the model's `KMSKeyArn` pattern, which the durable
/// function here names in its `DurableConfig`.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/durable-executions";

/// How long this suite waits for an accepted event to be delivered and the execution it
/// drives to end. The delivery happens on the runtime's own time, so the wait is a poll
/// rather than a single sleep.
///
/// The bound is what the EMULATOR itself allows one invocation, rather than a span picked
/// here: the wait for a share of the run capacity ([`PERMIT_WAIT`]) and the wait for the
/// run's own answer ([`HOST_WAIT_CEILING`]), with room for every attempt the retry policy has
/// for an event whose handler raised — separated by the waits AWS publishes between them
/// ([`FIRST_ASYNC_RETRY_DELAY`], [`SECOND_ASYNC_RETRY_DELAY`]) — and a margin for the queueing
/// either side of them. It is patience and not a weakened expectation — the assertions below
/// are unchanged — and a whole-workspace test run shares its cores with every other suite, so
/// a run really can wait for capacity before it starts.
const SETTLE_CEILING: Duration = PERMIT_WAIT
    .saturating_add(HOST_WAIT_CEILING)
    .saturating_add(FIRST_ASYNC_RETRY_DELAY)
    .saturating_add(SECOND_ASYNC_RETRY_DELAY)
    .saturating_add(Duration::from_secs(60));

/// How long one poll of the execution waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// `GetDurableExecution`: the label is not greedy, so the ARN's slashes are percent-encoded
/// in the path exactly as an SDK encodes them.
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

/// `GetDurableExecutionHistory` on one execution.
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

/// One row: how the handler ends, and the terminal status and history event the execution
/// must therefore reach.
struct Case {
    /// What makes this row its own case, and names its function and execution.
    case: &'static str,
    /// The handler's body.
    source: &'static str,
    /// The `ExecutionStatus` the execution must end in.
    status: &'static str,
    /// The history event type that end must record.
    event: &'static str,
    /// Text the event's own payload must carry — what the handler returned, or the error it
    /// raised.
    carries: &'static str,
}

/// An execution ends with what the invocation driving it came to, and a client polling it
/// sees that terminal status rather than waiting forever.
#[tokio::test]
async fn a_durable_execution_ends_with_what_its_invocation_came_to() {
    let rows = [
        Case {
            case: "a handler that returned",
            source: "def lambda_handler(event, context):\n    return {'step': 'done'}\n",
            status: "SUCCEEDED",
            event: "ExecutionSucceeded",
            carries: "done",
        },
        Case {
            case: "a handler that raised",
            source: "def lambda_handler(event, context):\n    raise ValueError('step broke')\n",
            status: "FAILED",
            event: "ExecutionFailed",
            carries: "step broke",
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        let name = format!("durable-{index}");
        let created = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "lambda_function.lambda_handler",
                "DurableConfig": { "KMSKeyArn": KMS_KEY, "ExecutionTimeout": 3_600 },
                "Code": { "ZipFile": base64_of(&package(row.source)) },
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: expected 201 from CreateFunction, got {} {}",
            row.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        tokio::time::sleep(ACTIVATION_DELAY + POLL_INTERVAL).await;
        // No RETRY for an errored handler: no row here is about how many delivery attempts an
        // event has ("the maximum number of times to retry when the function returns an
        // error", `MaximumRetryAttempts`, whose floor is 0), and Lambda waits minutes between
        // the attempts it makes, so a row left on the default would wait those out before the
        // execution could carry what its invocation came to.
        let configured = json_call(
            &router,
            "PUT",
            &format!("/2019-09-25/functions/{name}/event-invoke-config"),
            &json!({ "MaximumRetryAttempts": 0 }),
        )
        .await;
        assert_eq!(
            configured.status,
            200,
            "case {:?}: expected 200 from PutFunctionEventInvokeConfig, got {} {}",
            row.case,
            configured.status,
            String::from_utf8_lossy(&configured.body)
        );

        let execution = format!("exec-{index}");
        let invoked = send(
            &router,
            "POST",
            &format!("{INVOCATIONS_URI}/{name}/invocations"),
            "application/json",
            br#"{"marker":"durable"}"#.to_vec(),
            &[
                ("x-amz-invocation-type", "Event".to_string()),
                (DURABLE_EXECUTION_NAME_HEADER, execution.clone()),
            ],
        )
        .await;
        assert_eq!(
            invoked.status,
            202,
            "case {:?}: expected 202 from Invoke, got {} {}",
            row.case,
            invoked.status,
            String::from_utf8_lossy(&invoked.body)
        );
        let arn = invoked
            .header(DURABLE_EXECUTION_ARN_HEADER)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the accepted invocation to answer an execution ARN",
                    row.case
                )
            });

        // The execution is polled the way a client polls it: the delivery happens on the
        // runtime's own time, so the terminal status is waited FOR rather than assumed.
        let deadline = std::time::Instant::now() + SETTLE_CEILING;
        let mut reported = String::new();
        while std::time::Instant::now() < deadline {
            let read = get_execution(&router, &arn).await;
            assert_eq!(
                read.status,
                200,
                "case {:?}: expected 200 from GetDurableExecution, got {} {}",
                row.case,
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            reported = read
                .json()
                .get("Status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if reported != "RUNNING" {
                // An execution that has ended reports WHEN it ended.
                assert!(
                    read.json().get("EndTimestamp").is_some(),
                    "case {:?}: expected an EndTimestamp on an execution that ended, got {}",
                    row.case,
                    String::from_utf8_lossy(&read.body)
                );
                break;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        assert_eq!(
            reported, row.status,
            "case {:?}: expected the execution to end {:?}, got {reported:?}",
            row.case, row.status
        );

        // The history records what happened, and carries what the run produced.
        let history = get_history(&router, &arn).await;
        assert_eq!(
            history.status,
            200,
            "case {:?}: expected 200 from GetDurableExecutionHistory, got {} {}",
            row.case,
            history.status,
            String::from_utf8_lossy(&history.body)
        );
        let rendered = String::from_utf8_lossy(&history.body).to_string();
        for expected in [row.event, row.carries] {
            assert!(
                rendered.contains(expected),
                "case {:?}: expected the history to carry {expected:?}, got {rendered}",
                row.case
            );
        }
    }
}

/// A blob member: restJson1 carries one base64-encoded in a JSON body.
fn base64_of(bytes: &[u8]) -> Value {
    Value::String(::base64::Engine::encode(
        &::base64::engine::general_purpose::STANDARD,
        bytes,
    ))
}

/// The `X-Amz-Target` prefix the key service routes on.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// One call to the key service on the awsJson1_1 wire.
async fn kms_call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://kms.us-east-1.amazonaws.com/")
        .header("host", "kms.us-east-1.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=emulator-test-key/20260101/us-east-1/kms/aws4_request, \
             SignedHeaders=host, Signature=not-verified",
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{KMS_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// Reporting a durable execution's DATA decrypts it under the key the function's
/// `DurableConfig` names, so a key the key service reports as unusable refuses the read
/// with the modelled 502 — and a read that asks for no data decrypts nothing and is served.
///
/// The rows differ along what the rule generalises over: which read is made, and whether
/// the request asks for the execution's data at all.
#[tokio::test]
async fn a_read_of_execution_data_is_refused_while_its_key_cannot_be_used() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );

    // A key this account really holds, and really disables.
    let created = kms_call(&router, "CreateKey", &json!({})).await;
    assert_eq!(
        created.status,
        200,
        "expected 200 from CreateKey, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let key_arn = created
        .json()
        .pointer("/KeyMetadata/Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "expected CreateKey to answer a key ARN, got {}",
                String::from_utf8_lossy(&created.body)
            )
        })
        .to_string();

    let name = "durable-kms";
    let source = "def lambda_handler(event, context):\n    return {'step': 'done'}\n";
    let created_function = create_function(
        &router,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "DurableConfig": { "KMSKeyArn": key_arn, "ExecutionTimeout": 3_600 },
            "Code": { "ZipFile": base64_of(&package(source)) },
        }),
    )
    .await;
    assert_eq!(
        created_function.status,
        201,
        "expected 201 from CreateFunction, got {} {}",
        created_function.status,
        String::from_utf8_lossy(&created_function.body)
    );
    tokio::time::sleep(ACTIVATION_DELAY + POLL_INTERVAL).await;

    let invoked = send(
        &router,
        "POST",
        &format!("{INVOCATIONS_URI}/{name}/invocations"),
        "application/json",
        br#"{"marker":"kms"}"#.to_vec(),
        &[
            ("x-amz-invocation-type", "Event".to_string()),
            (DURABLE_EXECUTION_NAME_HEADER, "kms-exec".to_string()),
        ],
    )
    .await;
    assert_eq!(
        invoked.status,
        202,
        "expected 202 from Invoke, got {} {}",
        invoked.status,
        String::from_utf8_lossy(&invoked.body)
    );
    let arn = invoked
        .header(DURABLE_EXECUTION_ARN_HEADER)
        .unwrap_or_else(|| panic!("expected the accepted invocation to answer an execution ARN"));

    // While the key is usable every read is served.
    for (case, answer) in [
        ("the execution", get_execution(&router, &arn).await),
        ("its history", get_history(&router, &arn).await),
    ] {
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 while the key is usable, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    let disabled = kms_call(&router, "DisableKey", &json!({ "KeyId": key_arn })).await;
    assert_eq!(
        disabled.status,
        200,
        "expected 200 from DisableKey, got {} {}",
        disabled.status,
        String::from_utf8_lossy(&disabled.body)
    );

    // (case, the read, the status it must answer)
    let rows: [(&str, Answer, u16); 3] = [
        ("the execution's data", get_execution(&router, &arn).await, 502),
        ("its history", get_history(&router, &arn).await, 502),
        (
            "the execution's metadata alone",
            json_call(
                &router,
                "GET",
                &format!(
                    "/2025-12-01/durable-executions/{}?IncludeExecutionData=false",
                    arn.replace('/', "%2F")
                ),
                &Value::Null,
            )
            .await,
            200,
        ),
    ];
    for (case, answer, status) in rows {
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected {status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if status != 502 {
            continue;
        }
        assert_eq!(
            answer.error_code(),
            "KMSDisabledException",
            "case {case:?}: expected the modelled KMS refusal, got {}",
            answer.error_code()
        );
        let message = answer
            .string("Message")
            .or_else(|| answer.string("message"))
            .unwrap_or_default();
        assert!(
            message.contains("the KMS key") && message.contains(&key_arn),
            "case {case:?}: expected the disabled-key sentence to name {key_arn:?}, got \
             {message:?}"
        );
    }
}
