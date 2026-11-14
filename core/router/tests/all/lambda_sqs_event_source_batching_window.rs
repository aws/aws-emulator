// SPDX-License-Identifier: CC0-1.0

//! The BATCHING WINDOW of a Lambda event source mapping on an Amazon SQS queue, across the
//! two services that own the halves of it: Amazon SQS holds the queue and its records, and
//! Lambda polls that queue and decides when its function is handed what it took.
//!
//! This suite needs more than one service by construction: the window is observable only as
//! the relation between what SQS holds (how many records are in flight with the poller) and
//! what Lambda reports (that it made a processing attempt, and when), so no single-service
//! test can see it.
//!
//! The RULE asserted over the table below, in one sentence: a mapping whose
//! `MaximumBatchingWindowInSeconds` is a span of its own GATHERS the records that arrive over
//! that span into one batch — "the maximum amount of time, in seconds, that Lambda spends
//! gathering records before invoking the function" — so it takes each record out of the
//! source as it arrives but hands its function nothing until that span has run from the first
//! record it gathered, and then hands it every record of the window at once; a mapping that
//! spends no time gathering hands over what one poll found; and a batch that fills first is
//! handed over then, because the span is a MAXIMUM while `BatchSize` is "the maximum number
//! of records in each batch".
//!
//! Nothing here invokes the function and nothing runs a deployment package: each row's
//! function carries a package no engine can run, so the attempt the mapping makes FAILS —
//! which is what makes that attempt observable (`LastProcessingResult`) at the instant the
//! mapping made it, with its whole batch still in flight, because a failed attempt deletes
//! nothing. The only data-plane calls a row makes are the `SendMessage`s that put the records
//! on the queue: a mapping that gathered nothing, or handed over early, fails the row.
//!
//! The rows differ along what the rule generalises over: the window (none, which is the
//! published minimum of its `@range(min: 0, max: 300)`, a span between the bounds, and the
//! maximum), how many records there are and whether they are already waiting or arrive during
//! the window, and whether `BatchSize` fills before the window runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, POLL_INTERVAL,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the Region every ARN carries.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";
/// Where `CreateEventSourceMapping` is bound
/// (`POST /2015-03-31/event-source-mappings`).
pub(crate) const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/queue-consumer";

/// An opaque deployment package, base64-encoded as `FunctionCode$ZipFile` is on the wire. Its
/// contents are never interpreted and no engine in this emulator runs it, which is what makes
/// the attempt a mapping makes with it fail — the signal this suite reads.
const PACKAGE_ZIP_BASE64: &str = "cGFja2FnZQ==";

/// The `State` a mapping reports once it polls its event source.
pub(crate) const STATE_ENABLED: &str = "Enabled";

/// The attribute that says how many of a queue's records are in flight with a consumer:
/// "ApproximateNumberOfMessagesNotVisible" (`GetQueueAttributes$AttributeNames`).
pub(crate) const IN_FLIGHT_MESSAGES: &str = "ApproximateNumberOfMessagesNotVisible";

/// How long every queue below hides a record it hands over for, and the mapped function's own
/// `Timeout`: the queue has to outlast the function for the mapping to be made at all, and the
/// hold has to outlast a row so a record a failed attempt left behind does not come back
/// mid-row.
const VISIBILITY_TIMEOUT: i64 = 30;
/// See [`VISIBILITY_TIMEOUT`].
pub(crate) const FUNCTION_TIMEOUT: i64 = 1;

/// How long a row waits for work the clock owes it — the mapping settling, a batch gathered,
/// the attempt made — on top of the window it is waiting out. Well above the poll interval and
/// the settle span, so a row fails only when the work did not happen at all.
pub(crate) const BUDGET: Duration = Duration::from_secs(30);

/// How long between two looks while waiting for one of those.
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Lambda and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs` mounts them: the
/// records are sent where a client sends them and the mapping polls the queue in the service
/// that owns it.
fn emulator() -> MultiServiceRouter {
    shared_emulator()
}

/// [`emulator`], for the sibling suites that mount the same pair.
pub(crate) fn shared_emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDROUTERTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One Lambda `restJson1` call, as its status and parsed body.
pub(crate) async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };
    (status, parsed)
}

/// One Amazon SQS call (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`).
pub(crate) async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A queue called `name` hiding a delivered record for [`VISIBILITY_TIMEOUT`]; answers its url
/// and the ARN a mapping names it by, both read back out of Amazon SQS.
async fn given_queue(router: &MultiServiceRouter, case: &str, name: &str) -> (String, String) {
    given_queue_hiding_for(router, case, name, VISIBILITY_TIMEOUT).await
}

/// [`given_queue`] with the queue's own `VisibilityTimeout` named by the caller — for a suite
/// whose rule is about how that span relates to the span a consumer holds a record for.
pub(crate) async fn given_queue_hiding_for(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    visibility_timeout: i64,
) -> (String, String) {
    let (status, created) = sqs(
        router,
        "CreateQueue",
        &json!({
            "QueueName": name,
            "Attributes": { "VisibilityTimeout": visibility_timeout.to_string() },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue {name:?} answered {status} {created}"
    );
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {created}"))
        .to_string();
    let arn = queue_attribute(router, case, &url, "QueueArn")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the queue {name:?} reports no QueueArn"));
    (url, arn)
}

/// One attribute of the queue `url` addresses, as `GetQueueAttributes` reports it.
pub(crate) async fn queue_attribute(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    name: &str,
) -> Option<String> {
    let (status, read) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": [name] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetQueueAttributes for {name} answered {status} {read}"
    );
    read["Attributes"][name].as_str().map(str::to_string)
}

/// A function a mapping can be made on, whose deployment package is NOT one any engine runs —
/// so the attempt the mapping makes with it fails, which is what this suite reads.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str) {
    given_unrunnable_function(router, case, name).await;
}

/// [`given_function`], for the sibling suites that need the same unrunnable function.
pub(crate) async fn given_unrunnable_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        Some(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Timeout": FUNCTION_TIMEOUT,
            "Code": { "ZipFile": PACKAGE_ZIP_BASE64 },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction {name:?} answered {status} {created}"
    );
}

/// `SendMessage` of `body` to the queue `url` addresses.
pub(crate) async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage answered {status} {sent}"
    );
}

/// `GetEventSourceMapping` on the mapping `uuid` names.
pub(crate) async fn get_mapping(router: &MultiServiceRouter, uuid: &str) -> Value {
    let (status, read) = lambda(router, "GET", &format!("{MAPPINGS_URI}/{uuid}"), None).await;
    assert_eq!(
        status, 200,
        "GetEventSourceMapping {uuid} answered {status} {read}"
    );
    read
}

/// What the mapping `uuid` names reports as the result of its last processing attempt, or
/// `None` while it has made none.
pub(crate) async fn last_processing_result(router: &MultiServiceRouter, uuid: &str) -> Option<String> {
    get_mapping(router, uuid).await["LastProcessingResult"]
        .as_str()
        .map(str::to_string)
}

/// Wait until the mapping `uuid` names reports `state`.
pub(crate) async fn await_state(router: &MultiServiceRouter, case: &str, uuid: &str, state: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = get_mapping(router, uuid).await;
        if read["State"].as_str() == Some(state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the mapping must settle into {state}, got {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the mapping `uuid` names has made a processing attempt, answering the instant it
/// was first seen to have made one and what it reported — `None` when it made none inside
/// [`BUDGET`].
async fn await_attempt(router: &MultiServiceRouter, uuid: &str) -> Option<(Instant, String)> {
    let deadline = Instant::now() + BUDGET;
    loop {
        if let Some(result) = last_processing_result(router, uuid).await {
            return Some((Instant::now(), result));
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the window the mapping gathers over, how many records are waiting when the clock
/// starts, how many arrive during the window, the `BatchSize` they are polled under, and
/// whether the row expects the window to have been waited out.
struct Row {
    case: &'static str,
    window: Option<i64>,
    waiting: usize,
    arriving: usize,
    batch_size: i64,
    /// Whether the batch is handed over only once the window has run: false for a row whose
    /// `BatchSize` fills first, which is handed over then.
    waits_out_the_window: bool,
}

/// RULE: a mapping gathers over its `MaximumBatchingWindowInSeconds` and hands its function
/// what accumulated — no sooner than that span after the first record it gathered — unless its
/// `BatchSize` fills first, in which case the batch goes then.
#[tokio::test]
async fn a_batching_window_gathers_the_records_that_arrive_during_it() {
    let rows = [
        Row {
            case: "no-window-hands-over-what-one-poll-found",
            window: None,
            waiting: 1,
            arriving: 0,
            batch_size: 10,
            waits_out_the_window: false,
        },
        Row {
            case: "a-window-gathers-what-arrives-during-it",
            window: Some(3),
            waiting: 1,
            arriving: 2,
            batch_size: 10,
            waits_out_the_window: true,
        },
        Row {
            case: "a-full-batch-goes-before-the-window-runs",
            window: Some(300),
            waiting: 2,
            arriving: 0,
            batch_size: 2,
            waits_out_the_window: false,
        },
    ];
    let router = emulator();
    for row in &rows {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, case, &format!("{case}-source")).await;
        given_function(&router, case, case).await;

        let mut request = json!({
            "FunctionName": case,
            "EventSourceArn": source_arn,
            "BatchSize": row.batch_size,
            "Enabled": true,
        });
        if let Some(window) = row.window {
            request["MaximumBatchingWindowInSeconds"] = json!(window);
        }
        let (status, created) = lambda(&router, "POST", MAPPINGS_URI, Some(&request)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateEventSourceMapping answered {status} {created}"
        );
        let uuid = created["UUID"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {created}"))
            .to_string();
        await_state(&router, case, &uuid, STATE_ENABLED).await;
        assert_eq!(
            last_processing_result(&router, &uuid).await,
            None,
            "case {case:?}: a mapping with nothing on its queue must have made no processing \
             attempt"
        );

        // The clock starts with the records that are already waiting when the mapping is
        // polling, so the span a row measures is one the mapping spent gathering.
        let records = row.waiting + row.arriving;
        let started = Instant::now();
        for index in 0..row.waiting {
            send_message(&router, case, &source_url, &format!("{case}-{index}")).await;
        }
        // The rest arrive DURING the window, one poll interval apart, so they are gathered
        // into the batch the first record opened rather than into one of their own.
        for index in row.waiting..records {
            tokio::time::sleep(POLL_INTERVAL).await;
            send_message(&router, case, &source_url, &format!("{case}-{index}")).await;
        }

        let (seen, result) = await_attempt(&router, &uuid)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the mapping made no processing attempt"));
        let window = Duration::from_secs(row.window.unwrap_or(0).unsigned_abs());
        let elapsed = seen.saturating_duration_since(started);
        if row.waits_out_the_window {
            assert!(
                elapsed >= window,
                "case {case:?}: a mapping gathering over {window:?} must hand its function \
                 nothing before that span has run, it attempted after {elapsed:?}"
            );
        } else if !window.is_zero() {
            assert!(
                elapsed < window,
                "case {case:?}: a batch that fills its BatchSize goes without waiting the \
                 window out, it took {elapsed:?} of {window:?}"
            );
        }
        // The attempt the mapping made is one its function could not take — its package is not
        // one any engine runs — which is what leaves the whole batch it gathered in flight to
        // be counted.
        assert_eq!(
            result, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
            "case {case:?}: expected the attempt to report \
             {LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED}, got {result:?}"
        );
        let held: u32 = queue_attribute(&router, case, &source_url, IN_FLIGHT_MESSAGES)
            .await
            .and_then(|count| count.parse().ok())
            .unwrap_or_default();
        assert_eq!(
            u64::from(held),
            records as u64,
            "case {case:?}: expected the one batch the mapping handed over to hold every \
             record of the window ({records} of them), {held} were in flight with it"
        );
    }
}
