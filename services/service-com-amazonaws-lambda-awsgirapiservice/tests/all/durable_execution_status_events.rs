// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EventBridge events a DURABLE EXECUTION's status changes owe the
//! account's default event bus — Lambda, Amazon EventBridge and Amazon SQS on ONE engine,
//! with the published event read back where a client reads it: off the queue the matching
//! rule targets.
//!
//! The RULE pinned here: every status a durable execution MOVES INTO is published onto the
//! account's default bus as one event, under the `source` Lambda owns (`aws.lambda`) and the
//! `detail-type` of a durable execution's status change, whose `detail` names the execution,
//! the function it runs on, the status it moved into and the instants it started and (once it
//! has one) ended — so a rule on that bus delivers one envelope per status to its targets,
//! and a rule scoped to ONE status receives only that status. Before this held, a client
//! watching durable executions through EventBridge saw nothing at all: no delivery, no dead
//! letter and no error, so an execution that timed out looked exactly like one still running.
//!
//! The rows span the class rather than the one recorded case: they differ along HOW the
//! execution reaches its terminal status — a handler that returned (`SUCCEEDED`), a handler
//! that raised (`FAILED`), an execution that outlived its `DurableConfig.ExecutionTimeout`
//! (`TIMED_OUT`, which no caller asks for at all) and an execution a caller stopped
//! (`STOPPED`) — and each row asserts both the `RUNNING` event the start owes and the
//! terminal one, each exactly once.
//!
//! The package writer is the one `synchronous_invoke_run.rs` publishes, so there is one
//! writer of a deployment archive in this target.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{create_function, json_call, router_on, send, ROLE};
use crate::synchronous_invoke_run::package;

/// The `source` every event this suite reads must carry: EventBridge names the publishing
/// service in an event's `source` as `aws.<service>`, and Lambda's namespace is `lambda`.
/// Written out here rather than taken from the implementation's own constant, so the
/// published value is checked against an expectation of its own.
const EVENT_SOURCE: &str = "aws.lambda";

/// The `detail-type` every event this suite reads must carry, in the form AWS names a
/// resource's status change by — `<service> <resource> Status Change`, the shape this tree's
/// other publishers carry (`Step Functions Execution Status Change`,
/// `CloudFormation StackSet Status Change`). Written out here for the same reason as
/// [`EVENT_SOURCE`]: a rule matches on this string, so the test states it rather than reading
/// it back out of the code that publishes it.
const DURABLE_EXECUTION_STATUS_CHANGE: &str = "Lambda Durable Execution Status Change";

/// Where `Invoke` is bound.
const INVOCATIONS_URI: &str = "/2015-03-31/functions";

/// The request header `Invoke$DurableExecutionName` is bound to.
const DURABLE_EXECUTION_NAME_HEADER: &str = "x-amz-durable-execution-name";

/// The response header `Invoke$DurableExecutionArn` is bound to.
const DURABLE_EXECUTION_ARN_HEADER: &str = "x-amz-durable-execution-arn";

/// The Region every request below is signed for, which is the Region whose default bus the
/// events are published onto.
const REGION: &str = "us-east-1";

/// The rule that watches every durable-execution status change of this suite's functions, and
/// the queue it delivers to.
const ALL_STATUSES_RULE: &str = "durable-status-changes";
const ALL_STATUSES_QUEUE: &str = "durable-status-changes";

/// The rule scoped to ONE status, and its own queue: it must receive the `SUCCEEDED` events
/// and nothing else.
const SUCCEEDED_RULE: &str = "durable-succeeded-only";
const SUCCEEDED_QUEUE: &str = "durable-succeeded-only";

/// The service principal EventBridge delivers to an SQS target as, which the queue's own
/// policy grants: "for Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge
/// relies on resource-based policies" (`PutTargets`).
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// How long this suite waits for an accepted invocation to run, the execution it drives to
/// reach its terminal status and both events to be delivered. The publication happens off the
/// response path, on the clock the store runs, so every wait below is a POLL rather than a
/// single sleep.
///
/// A ceiling on PATIENCE: an execution whose handler RAISED ends only once the attempts the
/// retry policy has for its event are spent, and AWS publishes a minute-scale wait between
/// them, so this is far longer than the publication itself takes on any host.
const DELIVERY_CEILING: Duration = Duration::from_secs(300);

/// How long one poll of the queue waits before asking again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The `ExecutionTimeout` the timed-out row runs under, in seconds — the model's own floor
/// (`DurableConfig$ExecutionTimeout` is `@range(min: 1, max: 31622400)`), so the execution
/// outlives it while its handler is still waiting.
const SHORT_EXECUTION_TIMEOUT: i64 = 1;

/// The `ExecutionTimeout` every other row runs under: far longer than the row takes, so
/// nothing times out that the row is not about.
const LONG_EXECUTION_TIMEOUT: i64 = 3_600;

/// How long the handler of the rows that must still be running waits, in seconds.
const HANDLER_WAIT_SECONDS: u64 = 3;

/// The function `Timeout` those rows run under: longer than the handler waits, so the run is
/// not cut off before the status the row is about is reached.
const FUNCTION_TIMEOUT: i64 = 20;

/// One row: how the execution reaches its terminal status.
struct Case {
    /// What makes this row its own case, and names its function and execution.
    case: &'static str,
    /// The handler's body.
    source: &'static str,
    /// The `DurableConfig$ExecutionTimeout` the function runs under.
    execution_timeout: i64,
    /// Whether the row STOPS the execution once it is running, rather than letting the run
    /// decide it.
    stopped: bool,
    /// The `ExecutionStatus` the execution must end in, which is the status of the second
    /// event the rule must deliver.
    terminal: &'static str,
}

/// Every status a durable execution moves into reaches a rule on the account's default bus,
/// once each, carrying the execution's identity and instants.
#[tokio::test]
async fn every_durable_execution_status_change_reaches_a_rule_on_the_default_bus() {
    let rows = [
        Case {
            case: "a handler that returned",
            source: "def lambda_handler(event, context):\n    return {'step': 'done'}\n",
            execution_timeout: LONG_EXECUTION_TIMEOUT,
            stopped: false,
            terminal: "SUCCEEDED",
        },
        Case {
            case: "a handler that raised",
            source: "def lambda_handler(event, context):\n    raise ValueError('step broke')\n",
            execution_timeout: LONG_EXECUTION_TIMEOUT,
            stopped: false,
            terminal: "FAILED",
        },
        Case {
            case: "an execution that outlived its execution timeout",
            source: "import time\n\n\ndef lambda_handler(event, context):\n    \
                     time.sleep(event['wait_seconds'])\n    return {'step': 'late'}\n",
            execution_timeout: SHORT_EXECUTION_TIMEOUT,
            stopped: false,
            terminal: "TIMED_OUT",
        },
        Case {
            case: "an execution a caller stopped",
            source: "import time\n\n\ndef lambda_handler(event, context):\n    \
                     time.sleep(event['wait_seconds'])\n    return {'step': 'late'}\n",
            execution_timeout: LONG_EXECUTION_TIMEOUT,
            stopped: true,
            terminal: "STOPPED",
        },
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator(&engine);
    let all_statuses = control_queue(
        &router,
        ALL_STATUSES_QUEUE,
        ALL_STATUSES_RULE,
        &json!({ "source": [EVENT_SOURCE], "detail-type": [DURABLE_EXECUTION_STATUS_CHANGE] }),
    )
    .await;
    let succeeded_only = control_queue(
        &router,
        SUCCEEDED_QUEUE,
        SUCCEEDED_RULE,
        &json!({
            "source": [EVENT_SOURCE],
            "detail-type": [DURABLE_EXECUTION_STATUS_CHANGE],
            "detail": { "status": ["SUCCEEDED"] },
        }),
    )
    .await;

    // Every event this suite's executions published, keyed by the execution it is about and
    // the status it reports, so a row reads its own events out of the one queue the rule
    // delivers to and a second copy of one shows up as a count above 1.
    let mut delivered: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
    let mut arns: Vec<(&str, String, &str)> = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let name = format!("durable-events-{index}");
        let created = create_function(
            &router,
            &json!({
                "FunctionName": name,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "lambda_function.lambda_handler",
                "Timeout": FUNCTION_TIMEOUT,
                "DurableConfig": { "ExecutionTimeout": row.execution_timeout },
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
        // execution could reach the status it IS about.
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
            format!("{{\"wait_seconds\":{HANDLER_WAIT_SECONDS}}}").into_bytes(),
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

        // The start owes a `RUNNING` event, which is waited FOR: the publication happens on
        // the store's own clock rather than inside the call that was answered.
        collect_until(
            &router,
            &all_statuses,
            &mut delivered,
            row.case,
            &arn,
            "RUNNING",
        )
        .await;

        if row.stopped {
            let stop = json_call(
                &router,
                "POST",
                &format!(
                    "/2025-12-01/durable-executions/{}/stop",
                    arn.replace('/', "%2F")
                ),
                &json!({ "ErrorType": "OperatorStopped", "ErrorMessage": "stopped by the test" }),
            )
            .await;
            assert_eq!(
                stop.status,
                200,
                "case {:?}: expected 200 from StopDurableExecution, got {} {}",
                row.case,
                stop.status,
                String::from_utf8_lossy(&stop.body)
            );
        }
        arns.push((row.case, arn, row.terminal));
    }

    // Each execution's terminal status reaches the same rule, whichever of the four ways it
    // was reached — including the timeout, which no call asked for.
    for (case, arn, terminal) in &arns {
        collect_until(&router, &all_statuses, &mut delivered, case, arn, terminal).await;
    }

    for (case, arn, terminal) in &arns {
        for status in ["RUNNING", *terminal] {
            let events = delivered
                .get(&((*arn).clone(), status.to_string()))
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                events.len(),
                1,
                "case {case:?}: expected exactly one {status} event for {arn}, got \
                 {} ({events:?})",
                events.len()
            );
            let event = &events[0];
            assert_eq!(
                event["source"].as_str(),
                Some(EVENT_SOURCE),
                "case {case:?}: expected the {status} event to come from {EVENT_SOURCE}, got \
                 {event}"
            );
            let detail = &event["detail"];
            assert_eq!(
                detail["durableExecutionArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?}: expected the {status} event to name the execution, got {event}"
            );
            for member in ["durableExecutionName", "functionArn", "functionName"] {
                assert!(
                    detail[member].as_str().is_some(),
                    "case {case:?}: expected {member} in the {status} detail, got {event}"
                );
            }
            let started = detail["startTimestamp"].as_i64();
            assert!(
                started.is_some_and(|instant| instant > 0),
                "case {case:?}: expected a start instant in the {status} detail, got {event}"
            );
            // A terminal status reports WHEN the execution ended; one still running reports
            // no end instant at all, because nothing ended.
            let ended = detail["endTimestamp"].as_i64();
            if status == "RUNNING" {
                assert!(
                    ended.is_none(),
                    "case {case:?}: expected no end instant on a RUNNING event, got {event}"
                );
            } else {
                assert!(
                    ended.is_some_and(|instant| Some(instant) >= started),
                    "case {case:?}: expected an end instant at or after the start on the \
                     {status} event, got {event}"
                );
            }
        }
    }

    // The execution that outlived its `ExecutionTimeout` ended no earlier than that timeout
    // after it started: the timeout is what ended it, so the instant it ended is the instant
    // the timeout fell due.
    let timed_out = arns
        .iter()
        .find(|(_, _, terminal)| *terminal == "TIMED_OUT")
        .map(|(_, arn, terminal)| (arn.clone(), (*terminal).to_string()))
        .expect("the timed-out row is in the table");
    let event = &delivered[&timed_out][0];
    let started = event["detail"]["startTimestamp"].as_i64().unwrap_or_default();
    let ended = event["detail"]["endTimestamp"].as_i64().unwrap_or_default();
    assert!(
        ended - started >= SHORT_EXECUTION_TIMEOUT * 1_000,
        "expected the timed-out execution to end at least {SHORT_EXECUTION_TIMEOUT}s after it \
         started, got {ended} and {started} in {event}"
    );

    // A rule scoped to ONE status receives only that status: the queue of the SUCCEEDED-only
    // rule holds the one succeeding execution's event and nothing else. The expected one is
    // waited for the way every other delivery here is, so a moment's difference between the
    // two queues is not read as a missing event.
    let mut scoped: BTreeMap<(String, String), Vec<Value>> = BTreeMap::new();
    let (succeeding, succeeding_arn) = arns
        .iter()
        .find(|(_, _, terminal)| *terminal == "SUCCEEDED")
        .map(|(case, arn, _)| ((*case).to_string(), arn.clone()))
        .expect("the succeeding row is in the table");
    collect_until(
        &router,
        &succeeded_only,
        &mut scoped,
        &succeeding,
        &succeeding_arn,
        "SUCCEEDED",
    )
    .await;
    drain_into(&router, &succeeded_only, &mut scoped).await;
    let statuses: Vec<String> = scoped.keys().map(|(_, status)| status.clone()).collect();
    assert_eq!(
        statuses,
        vec!["SUCCEEDED".to_string()],
        "expected the SUCCEEDED-only rule to receive just that status, got {statuses:?}"
    );
}

/// Lambda, Amazon EventBridge and Amazon SQS over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the event Lambda publishes is matched by the bus and
/// delivered to the queue in this same process.
fn emulator(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = router_on(engine);
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(engine),
    ));
    router
}

/// One `awsJson` RPC call against the service whose signing name is `signing_name` — the
/// credential scope is what routes it — which must be served.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let answered: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&payload).to_string()))
    };
    assert_eq!(
        status, 200,
        "expected HTTP 200 from {operation}, got {status} with {answered}"
    );
    answered
}

/// The queue `queue` targeted by the rule `rule` on the DEFAULT bus matching `pattern`;
/// answers the queue's url.
async fn control_queue(
    router: &MultiServiceRouter,
    queue: &str,
    rule: &str,
    pattern: &Value,
) -> String {
    let created = rpc(
        router,
        "sqs",
        "AmazonSQS",
        "1.0",
        "CreateQueue",
        &json!({ "QueueName": queue }),
    )
    .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueUrl in {created}"))
        .to_string();
    let attributes = rpc(
        router,
        "sqs",
        "AmazonSQS",
        "1.0",
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn in {attributes}"))
        .to_string();
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowEventBridge",
            "Effect": "Allow",
            "Principal": { "Service": EVENTS_PRINCIPAL },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    })
    .to_string();
    rpc(
        router,
        "sqs",
        "AmazonSQS",
        "1.0",
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    rpc(
        router,
        "events",
        "AWSEvents",
        "1.1",
        "PutRule",
        &json!({ "Name": rule, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
    )
    .await;
    let targets = rpc(
        router,
        "events",
        "AWSEvents",
        "1.1",
        "PutTargets",
        &json!({ "Rule": rule, "Targets": [{ "Id": "t1", "Arn": arn }] }),
    )
    .await;
    assert_eq!(
        targets["FailedEntryCount"],
        json!(0),
        "PutTargets reported a failed entry: {targets}"
    );
    url
}

/// Take whatever is waiting on the queue at `url` and file each message under the execution
/// it is about and the status it reports.
async fn drain_into(
    router: &MultiServiceRouter,
    url: &str,
    into: &mut BTreeMap<(String, String), Vec<Value>>,
) {
    let received = rpc(
        router,
        "sqs",
        "AmazonSQS",
        "1.0",
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    for message in received["Messages"].as_array().cloned().unwrap_or_default() {
        let text = message["Body"].as_str().unwrap_or_default();
        let body: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let arn = body["detail"]["durableExecutionArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let status = body["detail"]["status"].as_str().unwrap_or_default().to_string();
        into.entry((arn, status)).or_default().push(body);
        // The message is taken off the queue, so a later poll reports what arrived SINCE
        // rather than handing this one over again.
        if let Some(receipt) = message["ReceiptHandle"].as_str() {
            rpc(
                router,
                "sqs",
                "AmazonSQS",
                "1.0",
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": receipt }),
            )
            .await;
        }
    }
}

/// Poll the queue at `url` until the event reporting `status` for `arn` has arrived, filing
/// everything that arrives meanwhile. Fails the case when nothing delivers it in time.
async fn collect_until(
    router: &MultiServiceRouter,
    url: &str,
    into: &mut BTreeMap<(String, String), Vec<Value>>,
    case: &str,
    arn: &str,
    status: &str,
) {
    let deadline = Instant::now() + DELIVERY_CEILING;
    while Instant::now() < deadline {
        if into.contains_key(&(arn.to_string(), status.to_string())) {
            return;
        }
        drain_into(router, url, into).await;
        if into.contains_key(&(arn.to_string(), status.to_string())) {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: no {status} status-change event was delivered for {arn} within \
         {DELIVERY_CEILING:?}; the queue held {:?}",
        into.keys().collect::<Vec<_>>()
    );
}

/// A blob member: restJson1 carries one base64-encoded in a JSON body.
fn base64_of(bytes: &[u8]) -> Value {
    Value::String(::base64::Engine::encode(
        &::base64::engine::general_purpose::STANDARD,
        bytes,
    ))
}
