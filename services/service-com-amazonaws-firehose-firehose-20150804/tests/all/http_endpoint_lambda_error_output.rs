// SPDX-License-Identifier: CC0-1.0

//! The error output of the records an HTTP endpoint destination REFUSED, where that endpoint
//! is another service of this emulator: a Lambda function URL whose handler answers the
//! delivery.
//!
//! The rule under test: when a destination's own answer REPORTS why it did not take a
//! delivery, that reported message is the `errorMessage` the error-output document of each
//! refused record carries — "only the documents that Firehose could not deliver to the
//! specified HTTP endpoint destination (`FailedDataOnly`)" (`S3BackupMode`) are parked, and
//! what a consumer of the `ErrorOutputPrefix` diagnoses a refusal by is the destination's own
//! account of it rather than a wording this emulator chose. The delivery protocol
//! `HttpEndpointConfiguration$Url` points at has a destination state that account in its
//! answer's own `errorMessage` member whatever the status line said, so an answer carrying
//! one is not an acknowledgment either way; an answer that states nothing there leaves the
//! failure class's own wording in its place. Each document also states how many attempts the
//! buffer really cost, which is more than one for a buffer attempted again inside
//! `RetryOptions$DurationInSeconds` ("the total amount of time that Firehose spends on
//! retries"), and a delivery the destination ACKNOWLEDGED is parked nowhere at all.
//!
//! Only an end-to-end run can pin this: the endpoint is a Lambda function URL served by this
//! same emulator, so the POST Firehose renders is the request a handler RUNS on, the answer
//! Firehose reads is that handler's own, and the parked objects are read back out of Amazon
//! S3 over its own wire. Nothing stands between the three services, and no transport of this
//! suite's own answers for the endpoint.
//!
//! The rows differ along what the rule generalises over: the status the endpoint answers
//! (four different server errors and a success), whether its answer reports a message at
//! all, and what that message says — its content, its length (including an account longer
//! than the bound on what one answer is copied into this process), and whether it carries
//! the quotes a JSON document has to escape.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::state::delivery::MAX_REPORTED_MESSAGE_CHARS;
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{REGION, ROLE_ARN};

/// The endpoint every control-plane request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// The bucket every destination below backs its refused records up to.
const BUCKET: &str = "firehose-http-lambda-backup";

/// `BucketARN` of [`BUCKET`], in the form that member's pattern requires.
pub(crate) const BUCKET_ARN: &str = "arn:aws:s3:::firehose-http-lambda-backup";

/// The `firehose:error-output-type` the records an HTTP endpoint destination did not place
/// are written under.
pub(crate) const FAILED_OUTPUT_TYPE: &str = "http-endpoint-failed";

/// The `errorCode` the error output names for a buffer the endpoint ANSWERED without a
/// success status.
pub(crate) const DESTINATION_EXCEPTION: &str = "HttpEndpoint.DestinationException";

/// The `errorCode` the error output names for a buffer the endpoint answered with a success
/// status that is no acknowledgment.
const INVALID_RESPONSE: &str = "HttpEndpoint.InvalidResponseFromDestination";

/// The function serving every row's endpoint, and the module and attribute its deployment
/// package carries the handler as.
const FUNCTION: &str = "firehose-http-endpoint-sink";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime the deployment package below is written for.
const RUNTIME: &str = "python3.12";

/// The function's own execution role, satisfying the model's `RoleArn` pattern.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long one invocation of that function is given, as its `Timeout`.
const FUNCTION_TIMEOUT: u64 = 20;

/// `RetryOptions$DurationInSeconds` every refused row configures: long enough that the buffer
/// is attempted again before it is parked, and short enough that a row does not wait on a
/// window it is not about.
pub(crate) const RETRY_SECONDS: i64 = 2;

/// How long an outcome is waited for before a row fails: far longer than the retry window
/// above and the emulator's own delivery span, so a loaded machine does not fail a row the
/// clock would have served.
pub(crate) const DEADLINE: Duration = Duration::from_secs(60);

/// How often an outcome is polled for while waiting.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The handler behind the function URL: it answers the status the delivery's own access key
/// names, with the acknowledgment envelope the delivery protocol asks for — the request's own
/// id and the instant the delivery named — and reports the access key's message in that
/// envelope's `errorMessage` member when the key carries one.
///
/// The access key is the one member of the destination configuration a case can carry an
/// instruction in (`HttpEndpointConfiguration$AccessKey`, "the access key required for
/// Kinesis Firehose to authenticate with the HTTP endpoint selected as the destination",
/// 0..4096 characters of anything), which is what lets every row be served by ONE function.
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    headers = event.get("headers") or {}
    directive = headers.get("x-amz-firehose-access-key") or "200:1:"
    parts = directive.split(":", 2)
    status = int(parts[0])
    repeat = int(parts[1]) if len(parts) > 1 and parts[1] else 1
    reported = (parts[2] if len(parts) > 2 else "") * repeat
    delivered = json.loads(event.get("body") or "{}")
    answer = {
        "requestId": headers.get("x-amz-firehose-request-id") or "",
        "timestamp": delivered.get("timestamp") or 0,
    }
    if reported:
        answer["errorMessage"] = reported
    return {
        "statusCode": status,
        "headers": {"content-type": "application/json"},
        "body": json.dumps(answer),
    }
"#;

/// A destination's own account of a refusal, as long as such an account reasonably gets, so
/// the rows span the LENGTH of what is copied as well as its content.
const LONG_REPORTED_MESSAGE: &str = concat!(
    "the sink refused this batch because its store would not take the records; ",
    "the batch was rejected whole rather than in part, so none of its records was indexed; ",
    "retry once the store reports capacity again, and consult the sink's own log for the ",
    "decision this answer reports",
);

/// What a row expects of its records once the destination has judged them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The destination took the buffer, so nothing of it is parked anywhere.
    Acknowledged,
    /// The destination did not take the buffer: its records are parked under this
    /// `errorCode`, carrying the message the endpoint reported.
    Parked { code: &'static str },
    /// The destination did not take the buffer and reported no message of its own, so the
    /// documents state the failure class's own wording.
    ParkedWithoutAReportedMessage { code: &'static str },
}

/// One row: how the endpoint answers this row's deliveries, and what becomes of them.
struct Row {
    case: &'static str,
    /// The status the endpoint answers, which the row's access key instructs it with.
    status: u16,
    /// The message the endpoint reports, empty for an answer that reports none.
    reported: &'static str,
    /// How many times over the endpoint reports that message, which is how a row reaches an
    /// account longer than the bound on what a document copies.
    repeat: usize,
    expect: Expected,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "refused-with-a-message",
            status: 500,
            reported: "the sink could not write this batch to its store",
            repeat: 1,
            expect: Expected::Parked {
                code: DESTINATION_EXCEPTION,
            },
        },
        Row {
            case: "unavailable-with-another-message",
            status: 503,
            // A message carrying the quotes a JSON document has to escape, so a document
            // that mangles rather than copies the destination's words fails here.
            reported: "quota exceeded for tenant \"acme\" - retry later",
            repeat: 1,
            expect: Expected::Parked {
                code: DESTINATION_EXCEPTION,
            },
        },
        Row {
            case: "success-status-reporting-an-error",
            status: 200,
            reported: "the sink parsed no record out of this batch",
            repeat: 1,
            expect: Expected::Parked {
                code: INVALID_RESPONSE,
            },
        },
        Row {
            case: "refused-with-a-long-message",
            status: 502,
            reported: LONG_REPORTED_MESSAGE,
            repeat: 1,
            expect: Expected::Parked {
                code: DESTINATION_EXCEPTION,
            },
        },
        Row {
            // An account longer than the bound on what one answer can add to this process,
            // of which the document carries as much as that bound admits.
            case: "refused-past-the-copied-bound",
            status: 429,
            reported: LONG_REPORTED_MESSAGE,
            repeat: 20,
            expect: Expected::Parked {
                code: DESTINATION_EXCEPTION,
            },
        },
        Row {
            case: "refused-without-a-message",
            status: 500,
            reported: "",
            repeat: 1,
            expect: Expected::ParkedWithoutAReportedMessage {
                code: DESTINATION_EXCEPTION,
            },
        },
        Row {
            case: "acknowledged",
            status: 200,
            reported: "",
            repeat: 1,
            expect: Expected::Acknowledged,
        },
    ]
}

impl Row {
    /// The `AccessKey` this row's destination carries, which is the instruction the handler
    /// answers it by.
    fn access_key(&self) -> String {
        format!("{}:{}:{}", self.status, self.repeat, self.reported)
    }

    /// The message this row's endpoint reports in its answer.
    fn reported_message(&self) -> String {
        self.reported.repeat(self.repeat)
    }

    /// What the error output of this row's records states as their message: the message the
    /// endpoint reported, as much of it as the bound on a copied message admits.
    fn expected_message(&self) -> String {
        self.reported_message()
            .chars()
            .take(MAX_REPORTED_MESSAGE_CHARS)
            .collect()
    }

    /// The records this row puts: distinct per row, so a document found under one row's
    /// prefix can only have come from that row's buffer.
    fn payloads(&self) -> Vec<Vec<u8>> {
        (0..2)
            .map(|index| format!("{}-record-{index}", self.case).into_bytes())
            .collect()
    }

    /// The prefix the row's refused records are written under: its own `ErrorOutputPrefix`
    /// with `!{firehose:error-output-type}` expanded.
    fn error_prefix(&self) -> String {
        format!("{}/err/{FAILED_OUTPUT_TYPE}/", self.case)
    }
}

// ---------------------------------------------------------------------------
// The emulator
// ---------------------------------------------------------------------------

/// Firehose, Lambda and Amazon S3 on ONE shared engine, exactly as the router mounts them:
/// the endpoint a destination names is a function URL Lambda serves in this same process, and
/// the destination's backup is the Amazon S3 service's own store.
///
/// IAM is deliberately not mounted, as in this crate's other delivery suites: a destination
/// role nothing can be asked about withholds nothing, so what a row observes is the delivery
/// itself.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// Dispatch one request as it arrives on the wire, answering its status and body.
    async fn send(
        &self,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, bytes::Bytes) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization(service));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.body().clone())
    }

    /// One Firehose `awsJson1_1` operation that must succeed.
    pub(crate) async fn firehose_ok(&self, case: &str, operation: &str, body: Value) -> Value {
        let (status, answered) = self
            .send(
                "firehose",
                "POST",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", &format!("Firehose_20150804.{operation}")),
                ],
                serde_json::to_vec(&body).expect("the request body serializes"),
            )
            .await;
        let text = String::from_utf8_lossy(&answered).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must succeed, got {status} {text}"
        );
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    /// One Lambda `restJson1` control-plane call, answering its status and JSON body.
    async fn lambda(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("the request body serializes"))
            .unwrap_or_default();
        let (status, answered) = self
            .send(
                "lambda",
                method,
                path,
                &[("content-type", "application/json")],
                payload,
            )
            .await;
        let text = String::from_utf8_lossy(&answered).to_string();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`, so the destinations' backup exists.
    ///
    /// The create names its `LocationConstraint`: a create that names none can only mean
    /// us-east-1, the Region the unspecified constraint stands for, so S3 refuses one sent to
    /// any other Region's endpoint and this harness signs for [`REGION`].
    pub(crate) async fn create_bucket(&self) {
        let (status, body) = self
            .send(
                "s3",
                "PUT",
                &format!("/{BUCKET}"),
                &[],
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .into_bytes(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "setup: the backup bucket must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Every key the backup bucket holds under `prefix`, read through `ListObjectsV2`.
    pub(crate) async fn keys_under(&self, case: &str, prefix: &str) -> Vec<String> {
        let (status, body) = self
            .send(
                "s3",
                "GET",
                &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
                &[],
                Vec::new(),
            )
            .await;
        let text = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: listing {prefix:?} must succeed, got {status} {text}"
        );
        let mut keys = keys_of(&text);
        keys.sort();
        keys
    }

    /// The bytes the backup bucket holds at `key`, read through `GetObject`.
    pub(crate) async fn object(&self, case: &str, key: &str) -> Vec<u8> {
        let (status, body) = self
            .send("s3", "GET", &format!("/{BUCKET}/{key}"), &[], Vec::new())
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: reading {key:?} must succeed, got {status} {}",
            String::from_utf8_lossy(&body)
        );
        body.to_vec()
    }

    /// How many records are still buffered for `stream`, read out of this service's own
    /// store: a buffer the destination acknowledged holds none of them any more.
    fn buffered(&self, stream: &str) -> usize {
        let state = self.engine.state::<
            service_com_amazonaws_firehose_firehose_20150804::ComAmazonawsFirehoseFirehose20150804State,
        >();
        let store = state.lock();
        store
            .streams
            .get(stream)
            .map(|stream| stream.records.len())
            .unwrap_or_default()
    }

    /// Wait for `ready` to hold, or fail naming `case` and what did not happen. Nothing here
    /// drives a delivery: what makes it hold is the emulator's own delivery clock, which is
    /// what a producer that makes no further call observes.
    async fn wait_until(&self, case: &str, expected: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + DEADLINE;
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {expected} within {DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The same, for a condition read over the wire.
    async fn wait_for_keys(&self, case: &str, prefix: &str) -> Vec<String> {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let keys = self.keys_under(case, prefix).await;
            if !keys.is_empty() {
                return keys;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the refused records under {prefix:?} within {DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential: this
/// harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Every `<Key>` of a `ListObjectsV2` answer, read with an XML reader rather than by slicing
/// the body.
fn keys_of(document: &str) -> Vec<String> {
    let mut reader = quick_xml::Reader::from_str(document);
    let mut keys = Vec::new();
    let mut inside = false;
    let mut current = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the listing must be well-formed XML: {error}"))
        {
            quick_xml::events::Event::Start(start) => {
                inside = start.name().as_ref() == b"Key";
                current.clear();
            }
            quick_xml::events::Event::Text(text) if inside => {
                current.push_str(&text.xml10_content().unwrap_or_default());
            }
            quick_xml::events::Event::End(end) => {
                if end.name().as_ref() == b"Key" {
                    keys.push(current.clone());
                }
                inside = false;
                current.clear();
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    keys
}

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function behind every row's endpoint, wait until Lambda reports it `Active`,
/// publish a public function URL for it and answer that URL.
pub(crate) async fn given_endpoint(emulator: &Emulator) -> String {
    let (status, created) = emulator
        .lambda(
            "POST",
            "/2015-03-31/functions",
            Some(json!({
                "FunctionName": FUNCTION,
                "Role": EXECUTION_ROLE,
                "Runtime": RUNTIME,
                "Handler": HANDLER,
                "Timeout": FUNCTION_TIMEOUT,
                "Code": {
                    "ZipFile": aws_smithy_types::base64::encode(package(HANDLER_SOURCE)),
                },
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "setup: CreateFunction must succeed, got {status} {created}"
    );

    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, reported) = emulator
            .lambda(
                "GET",
                &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
                None,
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: GetFunctionConfiguration must succeed, got {status} {reported}"
        );
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "setup: Lambda never reported {FUNCTION} Active within {DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let (status, url) = emulator
        .lambda(
            "POST",
            &format!("/2021-10-31/functions/{FUNCTION}/url"),
            Some(json!({ "AuthType": "NONE" })),
        )
        .await;
    assert_eq!(
        status, 201,
        "setup: CreateFunctionUrlConfig must succeed, got {status} {url}"
    );
    // An `AuthType=NONE` URL bypasses IAM authentication, and the function's own
    // resource-based policy is what admits the caller: without this grant the endpoint
    // refuses every anonymous request, including a delivery.
    let (status, granted) = emulator
        .lambda(
            "POST",
            &format!("/2015-03-31/functions/{FUNCTION}/policy"),
            Some(json!({
                "StatementId": "public-url",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": "*",
                "FunctionUrlAuthType": "NONE",
            })),
        )
        .await;
    assert_eq!(
        status, 201,
        "setup: AddPermission must succeed, got {status} {granted}"
    );
    url.get("FunctionUrl")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup: the create must answer a FunctionUrl, got {url}"))
        .to_string()
}

/// Wait until Firehose reports `stream` `ACTIVE`, which is when it takes a put: the create
/// leaves it `CREATING` and the clock settles it.
pub(crate) async fn await_active(emulator: &Emulator, stream: &str) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let described = emulator
            .firehose_ok(
                stream,
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": stream }),
            )
            .await;
        if described["DeliveryStreamDescription"]["DeliveryStreamStatus"].as_str()
            == Some("ACTIVE")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {stream:?}: the stream must become ACTIVE within {DEADLINE:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The destination configuration of `row`: the HTTP endpoint at `url`, answered by the access
/// key the row instructs the handler with, backing its refused records up under the row's own
/// error output prefix.
fn destination(row: &Row, url: &str) -> Value {
    json!({
        "EndpointConfiguration": {
            "Url": url,
            "Name": "the-lambda-sink",
            "AccessKey": row.access_key(),
        },
        // The shortest buffering the hints admit, so a row waits for the emulator's own
        // delivery span and nothing longer.
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        "RetryOptions": { "DurationInSeconds": RETRY_SECONDS },
        "S3BackupMode": "FailedDataOnly",
        "S3Configuration": {
            "RoleARN": ROLE_ARN,
            "BucketARN": BUCKET_ARN,
            "Prefix": format!("{}/backup/", row.case),
            "ErrorOutputPrefix": format!("{}/err/!{{firehose:error-output-type}}/", row.case),
        },
    })
}

#[tokio::test]
async fn the_error_output_of_a_refused_batch_carries_the_message_the_endpoint_reported() {
    let emulator = Emulator::new();
    emulator.create_bucket().await;
    let url = given_endpoint(&emulator).await;
    let rows = rows();

    // Every row's stream is created first, so the one `CREATING` -> `ACTIVE` span covers the
    // whole table.
    for row in &rows {
        emulator
            .firehose_ok(
                row.case,
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": row.case,
                    "DeliveryStreamType": "DirectPut",
                    "HttpEndpointDestinationConfiguration": destination(row, &url),
                }),
            )
            .await;
    }
    for row in &rows {
        await_active(&emulator, row.case).await;
    }

    // Every row's records are put before any outcome is read, so the rows' retry windows run
    // beside each other rather than one after the other.
    for row in &rows {
        let entries: Vec<Value> = row
            .payloads()
            .iter()
            .map(|data| json!({ "Data": aws_smithy_types::base64::encode(data) }))
            .collect();
        let answered = emulator
            .firehose_ok(
                row.case,
                "PutRecordBatch",
                json!({ "DeliveryStreamName": row.case, "Records": entries }),
            )
            .await;
        assert_eq!(
            answered["FailedPutCount"].as_i64(),
            Some(0),
            "case {:?}: every record of the batch must be accepted, got {answered}",
            row.case
        );
    }

    for row in &rows {
        let case = row.case;
        let (code, reported) = match row.expect {
            Expected::Acknowledged => {
                // The endpoint acknowledged this buffer, so the records are gone from it —
                // only an answer that acknowledges the request hands a buffer over.
                emulator
                    .wait_until(case, "the acknowledged buffer is handed over", || {
                        emulator.buffered(case) == 0
                    })
                    .await;
                continue;
            }
            Expected::Parked { code } => (code, Some(row.expected_message())),
            Expected::ParkedWithoutAReportedMessage { code } => (code, None),
        };

        let prefix = row.error_prefix();
        let keys = emulator.wait_for_keys(case, &prefix).await;
        let mut documents = Vec::new();
        for key in &keys {
            assert!(
                key.starts_with(&prefix),
                "case {case:?}: the refused records are written under {prefix:?}, got {key:?}"
            );
            let body = emulator.object(case, key).await;
            for line in String::from_utf8_lossy(&body).lines() {
                documents.push(serde_json::from_str::<Value>(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: every parked line is a JSON document, got {error}")
                }));
            }
        }
        let payloads = row.payloads();
        assert_eq!(
            documents.len(),
            payloads.len(),
            "case {case:?}: one document per refused record, got {documents:?}"
        );
        for (document, payload) in documents.iter().zip(&payloads) {
            let answered_code = document["errorCode"].as_str().unwrap_or_default();
            assert_eq!(
                answered_code, code,
                "case {case:?}: the document names how the delivery failed, expected \
                 {code:?}, got {document}"
            );
            let message = document["errorMessage"].as_str().unwrap_or_default();
            match &reported {
                Some(expected) => assert_eq!(
                    message, expected,
                    "case {case:?}: the document carries the message the endpoint reported, \
                     expected {expected:?}, got {message:?}"
                ),
                None => {
                    assert!(
                        !message.is_empty(),
                        "case {case:?}: a destination reporting no message leaves the failure \
                         class's own wording, got {message:?}"
                    );
                    for other in &rows {
                        let reported = other.reported_message();
                        assert!(
                            reported.is_empty() || message != reported,
                            "case {case:?}: no endpoint reported a message for this delivery, \
                             got {message:?}"
                        );
                    }
                }
            }
            let raw = document["rawData"].as_str().unwrap_or_default();
            let decoded = aws_smithy_types::base64::decode(raw).unwrap_or_else(|error| {
                panic!("case {case:?}: rawData is base64, got {error} for {raw:?}")
            });
            assert_eq!(
                decoded, *payload,
                "case {case:?}: the document carries the record's own bytes, got {:?}",
                String::from_utf8_lossy(&decoded)
            );
            // The buffer was attempted when it came due and again inside the retry window
            // this row configured, so the document states more than the one attempt a
            // first-try failure costs.
            let attempts = document["attemptsMade"].as_u64().unwrap_or_default();
            assert!(
                attempts >= 2,
                "case {case:?}: the document states the attempts the buffer cost over its \
                 {RETRY_SECONDS}s retry window, expected at least 2, got {attempts}"
            );
            assert!(
                document["arrivalTimestamp"].as_u64().unwrap_or_default() > 0
                    && document["attemptEndingTimestamp"]
                        .as_u64()
                        .unwrap_or_default()
                        > 0,
                "case {case:?}: the document says when the record arrived and when the \
                 attempt ended, got {document}"
            );
        }
    }

    // The row the endpoint acknowledged has nothing parked and nothing backed up: under
    // `FailedDataOnly` the Amazon S3 location takes only what the destination refused, and by
    // now every other row's window has been spent.
    for row in &rows {
        if row.expect != Expected::Acknowledged {
            continue;
        }
        let keys = emulator.keys_under(row.case, &format!("{}/", row.case)).await;
        assert!(
            keys.is_empty(),
            "case {:?}: an acknowledged delivery is parked nowhere, got {keys:?}",
            row.case
        );
    }
}
