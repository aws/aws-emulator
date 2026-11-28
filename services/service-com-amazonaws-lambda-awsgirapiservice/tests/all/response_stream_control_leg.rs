// SPDX-License-Identifier: CC0-1.0

//! What a RESPONSE-STREAMING invocation has already done by the time its stream ends: the
//! chunks the handler wrote, and the work it performed through the AWS SDK its managed runtime
//! supplies, read back in the service that owns it.
//!
//! RULE: a streaming invocation's answer is the bytes the handler wrote, in the order it wrote
//! them, each write its own `PayloadChunk`, and a stream that has carried bytes cannot be
//! withdrawn — so a handler that fails after its writes delivers those bytes and reports the
//! failure in the terminating `InvokeComplete` event. Everything the handler DID before it
//! failed stands too: a call it made through the SDK its runtime supplies has taken effect in
//! the service that owns it, so a side channel the handler reports itself on carries every
//! record it wrote, whether the handler then returned or raised.
//!
//! The rows differ along what the rule generalises over: how the handler ends (returning,
//! raising after its writes, ending the stream itself) and therefore whether the terminating
//! event names an error — the records on both legs are the same in every row, because the
//! handler performed the same work.
//!
//! The queue the handler reports on and the function are mounted on ONE engine exactly as
//! `core/router/src/lib.rs` assembles them, so the records read back are the ones Amazon SQS
//! holds.
//!
//! The suite RUNS handlers, so it takes the one invocation stream the suites that run handlers
//! share ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aws_smithy_eventstream::frame::{DecodedFrame, MessageFrameDecoder};
use aws_smithy_types::event_stream::HeaderValue;
use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's client is built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` these functions carry: a managed Node release that bundles the SDK and
/// supports response streaming.
const RUNTIME: &str = "nodejs22.x";

/// How long one invocation is given, as the function's own `Timeout`: well past what any run
/// needs on any host, because what is under test is what the run DID and not how fast an
/// engine gets through it.
const FUNCTION_TIMEOUT: i64 = 120;

/// How long a clock-owed move (the function becoming `Active`, a message arriving on the
/// queue) is waited for before the wait is called a failure. Generous: what is under test is
/// the move, never how fast this host makes it.
const CLOCK_BUDGET: Duration = Duration::from_secs(60);

/// How long the poll loops below wait before looking again.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// How many records each row's handler writes, on both legs.
const RECORD_COUNT: i64 = 3;

/// Lambda, Amazon SQS and CloudWatch Logs over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call and the delivery of the run's
/// log are both dispatched back into this same router, so the queue and the group this suite
/// reads are the ones they reached.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One decoded event-stream frame: the union member it carries and its payload.
#[derive(Debug)]
struct Frame {
    event_type: Option<String>,
    payload: Vec<u8>,
}

impl Frame {
    /// The frame's payload as JSON, when it is JSON at all.
    fn json(&self) -> Option<Value> {
        serde_json::from_slice(&self.payload).ok()
    }
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The frames of the event stream this answer's body is.
    fn frames(&self, case: &str) -> Vec<Frame> {
        let mut decoder = MessageFrameDecoder::new();
        let mut remaining: &[u8] = &self.body;
        let mut frames = Vec::new();
        while !remaining.is_empty() {
            match decoder.decode_frame(&mut remaining) {
                Ok(DecodedFrame::Complete(message)) => {
                    let event_type = message
                        .headers()
                        .iter()
                        .find(|header| header.name().as_str() == ":event-type")
                        .and_then(|header| match header.value() {
                            HeaderValue::String(value) => Some(value.as_str().to_string()),
                            _ => None,
                        });
                    frames.push(Frame {
                        event_type,
                        payload: message.payload().to_vec(),
                    });
                }
                Ok(DecodedFrame::Incomplete) => {
                    panic!("case {case:?}: the event stream ends mid-frame")
                }
                Err(error) => panic!(
                    "case {case:?}: the event stream is not decodable: {error}: {:?}",
                    self.text()
                ),
            }
        }
        frames
    }
}

/// Dispatch one Lambda `restJson1` request.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(
        parts.status.as_u16(),
        200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {} with {parsed}",
        parts.status.as_u16()
    );
    parsed
}

/// A .zip deployment package holding `source` as `index.mjs` — one ES module and nothing else,
/// so every `@aws-sdk/*` specifier in it is the runtime's to resolve.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("index.mjs", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function and WAIT for the activation the clock owes it, looking again until it
/// reports `Active`.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str, source: &str) {
    let created = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": FUNCTION_TIMEOUT,
            "MemorySize": 128,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {} {}",
        created.status,
        created.text()
    );
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let read = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            b"",
        )
        .await;
        let state = read
            .json()
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if state == "Active" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never became Active, got {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The records the queue holds, waiting until as many as `owed` have arrived or the budget
/// runs out — one control leg the handler reports itself on, read back in the service that
/// owns it.
async fn queued_records(
    router: &MultiServiceRouter,
    case: &str,
    queue_url: &str,
    owed: usize,
) -> Vec<i64> {
    let deadline = Instant::now() + CLOCK_BUDGET;
    let mut seen: Vec<i64> = Vec::new();
    loop {
        let received = sqs(
            router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        for message in received
            .get("Messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let body = message
                .get("Body")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(seq) = seq_of(&body) {
                seen.push(seq);
            }
        }
        if seen.len() >= owed || Instant::now() >= deadline {
            seen.sort_unstable();
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The records the function's own LOG GROUP holds, waiting until as many as `owed` have
/// arrived or the budget runs out — the other control leg, which is every line the handler
/// wrote while the invocation ran.
async fn logged_records(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    owed: usize,
) -> (Vec<i64>, Vec<String>) {
    let group = format!("/aws/lambda/{function}");
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let found = logs(
            router,
            case,
            "FilterLogEvents",
            &json!({ "logGroupName": group }),
        )
        .await;
        let messages: Vec<String> = found
            .get("events")
            .and_then(Value::as_array)
            .map(|events| {
                events
                    .iter()
                    .filter_map(|event| event.get("message").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut seen: Vec<i64> = messages
            .iter()
            .filter_map(|message| {
                // A record the runtime framed carries the instant, the invocation and the
                // level in front of the text the handler wrote, tab separated, which is the
                // frame a managed runtime delivers a console line in.
                message.rsplit('\t').next().and_then(seq_of)
            })
            .collect();
        if seen.len() >= owed || Instant::now() >= deadline {
            seen.sort_unstable();
            return (seen, messages);
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The sequence number the record `text` carries, for a line that is one of the handler's
/// records at all: every leg carries the same record, so every leg is read the same way.
///
/// A record reaches a leg either as the JSON document it is or, where the leg's own values are
/// constrained (a tag value is `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`), as the `seq=<n>` pair that
/// document's one member is.
fn seq_of(text: &str) -> Option<i64> {
    let text = text.trim();
    if let Some(number) = text.strip_prefix("seq=") {
        return number.parse().ok();
    }
    serde_json::from_str::<Value>(text)
        .ok()?
        .get("seq")
        .and_then(Value::as_i64)
}

/// The records the function's own TAGS hold, waiting until as many as `owed` have arrived or
/// the budget runs out — the control leg a handler that reports itself through the service it
/// runs on writes to.
async fn tagged_records(
    router: &MultiServiceRouter,
    function_arn: &str,
    owed: usize,
) -> (Vec<i64>, Value) {
    let deadline = Instant::now() + CLOCK_BUDGET;
    loop {
        let read = lambda(
            router,
            "GET",
            &format!("/2017-03-31/tags/{}", function_arn.replace(':', "%3A")),
            b"",
        )
        .await;
        let tags = read.json();
        let mut seen: Vec<i64> = tags
            .get("Tags")
            .and_then(Value::as_object)
            .map(|held| {
                held.values()
                    .filter_map(Value::as_str)
                    .filter_map(seq_of)
                    .collect()
            })
            .unwrap_or_default();
        if seen.len() >= owed || Instant::now() >= deadline {
            seen.sort_unstable();
            return (seen, tags);
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One CloudWatch Logs call that must answer 200.
async fn logs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("logs"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    assert_eq!(
        parts.status.as_u16(),
        200,
        "case {case:?}: expected HTTP 200 from CloudWatch Logs {operation}, got {} with {parsed}",
        parts.status.as_u16()
    );
    parsed
}

/// The handler every row runs: write one record per sequence number to the response stream AND
/// report the same record on the control leg through the SDK the runtime supplies, then end the
/// way the event says.
///
/// The ending is named by the event, so one package serves every row.
const HANDLER_SOURCE: &str = r#"
import { SQSClient, SendMessageCommand } from "@aws-sdk/client-sqs";
import { LambdaClient, TagResourceCommand } from "@aws-sdk/client-lambda";

// The records this EXECUTION ENVIRONMENT has seen, which an invocation served in it after
// the writes reads back: module scope is the environment's, not one invocation's.
const records = [];

export const handler = awslambda.streamifyResponse(async (event, responseStream, context) => {
    if (event.ending === "observe") {
        responseStream.write(JSON.stringify(records));
        return;
    }
    const client = new SQSClient({ region: event.region });
    const control = new LambdaClient({ region: event.region });
    for (let seq = 1; seq <= event.records; seq += 1) {
        const record = JSON.stringify({ seq: seq });
        records.push({ seq: seq });
        responseStream.write(record);
        console.log(record);
        await client.send(new SendMessageCommand({
            QueueUrl: event.queueUrl,
            MessageBody: record,
        }));
        await control.send(new TagResourceCommand({
            Resource: context.invokedFunctionArn,
            Tags: { ["record-" + seq]: "seq=" + seq },
        }));
    }
    if (event.ending === "raise") {
        throw new Error("after the last chunk");
    }
    if (event.ending === "end") {
        responseStream.end();
    }
});
"#;

/// One row: how the handler ends, and the `ErrorCode` the terminating event must name.
struct Row {
    case: &'static str,
    ending: &'static str,
    error_code: Option<&'static str>,
}

/// RULE: every write a streaming handler made reaches the client as its own `PayloadChunk`,
/// the stream always ends with the `InvokeComplete` event — naming the error of a handler that
/// failed after its writes — and every record the handler reported on the control leg is in
/// the service that owns it when the stream has ended.
#[tokio::test]
async fn a_streaming_invocation_delivers_its_chunks_and_the_work_its_handler_did() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let router = router();

    let rows: &[Row] = &[
        Row {
            case: "a handler that returns after its writes",
            ending: "return",
            error_code: None,
        },
        Row {
            case: "a handler that raises after its writes",
            ending: "raise",
            error_code: Some("Unhandled"),
        },
        Row {
            case: "a handler that ends the stream itself",
            ending: "end",
            error_code: None,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let queue_name = format!("stream-control-leg-{index}");
        let created = sqs(
            &router,
            case,
            "CreateQueue",
            &json!({ "QueueName": queue_name }),
        )
        .await;
        let queue_url = created
            .get("QueueUrl")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();
        let function = format!("stream-control-leg-{index}");
        create_function(&router, case, &function, HANDLER_SOURCE).await;

        let event = json!({
            "region": REGION,
            "queueUrl": queue_url,
            "records": RECORD_COUNT,
            "ending": row.ending,
        });
        let answer = lambda(
            &router,
            "POST",
            &format!("/2021-11-15/functions/{function}/response-streaming-invocations"),
            event.to_string().as_bytes(),
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a streaming invocation is answered 200, got {} {}",
            answer.status,
            answer.text()
        );

        let frames = answer.frames(case);
        let chunks: Vec<i64> = frames
            .iter()
            .filter(|frame| frame.event_type.as_deref() == Some("PayloadChunk"))
            .filter_map(|frame| frame.json())
            .filter_map(|record| record.get("seq").and_then(Value::as_i64))
            .collect();
        let owed: Vec<i64> = (1..=RECORD_COUNT).collect();
        assert_eq!(
            chunks, owed,
            "case {case:?}: expected one chunk per handler write {owed:?}, got {chunks:?} out \
             of {frames:?}"
        );

        let complete: Vec<&Frame> = frames
            .iter()
            .filter(|frame| frame.event_type.as_deref() == Some("InvokeComplete"))
            .collect();
        assert_eq!(
            complete.len(),
            1,
            "case {case:?}: the stream ends with exactly one InvokeComplete, got {frames:?}"
        );
        let reported = complete[0]
            .json()
            .and_then(|document| {
                document
                    .get("ErrorCode")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        match row.error_code {
            Some(expected) => assert_eq!(
                reported,
                expected,
                "case {case:?}: the terminating event names the handler's failure, got {:?}",
                complete[0].json()
            ),
            None => assert!(
                reported.is_empty(),
                "case {case:?}: a handler that completed names no error, got {:?}",
                complete[0].json()
            ),
        }

        let owed_count = usize::try_from(RECORD_COUNT).expect("the record count fits");
        let queued = queued_records(&router, case, &queue_url, owed_count).await;
        assert_eq!(
            queued, owed,
            "case {case:?}: expected the queue leg to hold every record the handler reported \
             {owed:?}, got {queued:?}"
        );
        let (logged, messages) = logged_records(&router, case, &function, owed_count).await;
        assert_eq!(
            logged, owed,
            "case {case:?}: expected the log leg to hold every record the handler wrote \
             {owed:?}, got {logged:?} out of {messages:?}"
        );
        let function_arn = format!("arn:aws:lambda:{REGION}:000000000000:function:{function}");
        let (tagged, tags) = tagged_records(&router, &function_arn, owed_count).await;
        assert_eq!(
            tagged, owed,
            "case {case:?}: expected the tag leg to hold every record the handler reported \
             {owed:?}, got {tagged:?} out of {tags}"
        );

        // The ENVIRONMENT the writes happened in is the one the next invocation of the
        // function is served in, so an invocation that only reports what that environment has
        // seen reports every record the writes left in it.
        let observed = lambda(
            &router,
            "POST",
            &format!("/2021-11-15/functions/{function}/response-streaming-invocations"),
            json!({ "ending": "observe" }).to_string().as_bytes(),
        )
        .await;
        let seen: Vec<i64> = observed
            .frames(case)
            .iter()
            .filter(|frame| frame.event_type.as_deref() == Some("PayloadChunk"))
            .filter_map(|frame| frame.json())
            .filter_map(|held| held.as_array().cloned())
            .flatten()
            .filter_map(|record| record.get("seq").and_then(Value::as_i64))
            .collect();
        assert_eq!(
            seen, owed,
            "case {case:?}: expected the environment that served the writes to still hold \
             {owed:?}, got {seen:?} out of {}",
            observed.text()
        );
    }
}
