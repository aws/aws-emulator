// SPDX-License-Identifier: CC0-1.0

//! What a RUNNING `python3.x` handler reads back for a member the called service's model types
//! as a TIMESTAMP: Lambda, Amazon S3, Amazon DynamoDB, Secrets Manager, CloudWatch Logs, IAM and
//! the token service on ONE engine, a handler whose first statement is `import boto3`, and every
//! value it reports compared with what the SAME call answers where a client reads it, at the
//! resolution the two readings of one instant share.
//!
//! RULE: the AWS SDK a managed runtime bundles hands the caller a native `datetime`, in UTC, for
//! every member of an answer the called service's model types as a `timestamp` — a JSON protocol
//! carries one as the seconds since the epoch, whole or fractional, and an SDK materialises
//! exactly the members the model says are instants — so a handler reads `.isoformat()`,
//! `.timestamp()`, `.microsecond` and the comparison of two instants off such a member, and a
//! member the model types as anything else (a `long` carrying an epoch, a count) is handed over
//! exactly as the wire carried it.
//!
//! The rows differ along what the rule generalises over: WHICH service answers and under which
//! protocol (`awsJson1_0`, `awsJson1_1`, and a `restXml` service whose answer this runtime builds
//! from its own client's output), WHERE the member sits in the answer (a top-level member, a
//! member of a nested structure, a member of a structure inside a list), and WHAT the model types
//! the value as (a timestamp, against a `long` of epoch milliseconds and a count in the same
//! answers). No row states an instant of its own: every expected value is the one the same
//! resource reports where a client reads it, and the sub-second part the `datetime` holds is
//! asserted to be the one its own epoch value carries — nothing was truncated on the way into it.
//! That the SEAM carries a sub-second fraction at all is pinned where two readings of ONE instant
//! are compared on the wire
//! (`core/router/tests/all/cloudfront_invalidation_response_propagates_to_its_queue.rs`).
//!
//! [`super::node_handler_sdk_answer_instants`] pins the same rule for the other SDK this runtime
//! bundles; this suite is the Python half, and the two differ in nothing but the language, which
//! is why the rows are the same shape.
//!
//! The suite RUNS handlers, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::element;
use crate::event_source_mappings::{await_function_active, call, router_on};
use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefixes of the AWS JSON services this suite reads on the wire (each
/// service's own `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const SECRETS_TARGET_PREFIX: &str = "secretsmanager";

/// The `Timeout` the function carries, in seconds: far past what a run needs, because what is
/// under test is the form a value reaches the guest in and never how fast a loaded host gets an
/// interpreter through a handler.
const FUNCTION_TIMEOUT_SECONDS: u64 = 120;

/// How long a wait for a clock-owed transition is given before the case is a failure, and how
/// long between two looks. The loop exits the instant its CONDITION holds, so the product is a
/// deadline and not a wait.
const BUDGET: Duration = Duration::from_secs(180);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// The bucket the `restXml` row lists, and the one object in it.
const BUCKET: &str = "python-handler-instant-objects";
const KEY: &str = "assets/held.txt";

/// The error code DynamoDB refuses a read of a table it does not hold with
/// (`ResourceNotFoundException`, HTTP 400).
const ABSENT_TABLE_CODE: &str = "ResourceNotFoundException";

/// The handler this suite runs: it makes the call each probe names with the SDK the runtime
/// bundles and reports WHAT FORM the value at the probe's path reached it in, so the
/// invocation's payload IS what the SDK gave the guest.
const HANDLER_SOURCE: &str = r#"
import datetime

import boto3
import botocore.exceptions


def _reached(answered, path):
    held = answered
    for step in path:
        if held is None:
            return None
        try:
            held = held[step]
        except (KeyError, IndexError, TypeError):
            return None
    return held


def lambda_handler(event, context):
    reports = []
    for probe in event["probes"]:
        client = boto3.client(probe["service"])
        try:
            answered = getattr(client, probe["method"])(**probe["input"])
        except botocore.exceptions.ClientError as error:
            reports.append({
                "ok": False,
                "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"],
            })
            continue
        value = _reached(answered, probe["path"])
        report = {"ok": True, "type": type(value).__name__}
        if isinstance(value, datetime.datetime):
            offset = value.utcoffset()
            report["epoch"] = value.timestamp()
            report["iso"] = value.isoformat()
            report["microsecond"] = value.microsecond
            report["offset"] = None if offset is None else offset.total_seconds()
            # An instant is COMPARED with another instant, which is what a handler that acts on
            # a reported date does: a value handed over as a number could not be.
            report["past"] = value < datetime.datetime.now(datetime.timezone.utc)
        else:
            report["plain"] = value
        reports.append(report)
    return {"reports": reports}
"#;

/// Lambda and the services the handler calls, over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so every value it reads back is the one this suite reads where a client reads
/// it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One AWS JSON call to a service of this emulator that must answer 200, as its parsed body.
async fn aws_json(
    router: &MultiServiceRouter,
    service: &str,
    content_type: &str,
    target: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(service))
        .header("content-type", content_type)
        .header("x-amz-target", target)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered).unwrap_or(Value::Null)
    };
    assert_eq!(
        parts.status.as_u16(),
        200,
        "setup: expected HTTP 200 from {service} {target}, got {} with {parsed}",
        parts.status
    );
    parsed
}

/// One DynamoDB call (`awsJson1_0`) that must answer 200.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    aws_json(
        router,
        "dynamodb",
        "application/x-amz-json-1.0",
        &format!("{DYNAMODB_TARGET_PREFIX}.{operation}"),
        body,
    )
    .await
}

/// One Amazon S3 REST call, path style, that must be served — answering the body as text,
/// which is the XML a client reads.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> String {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    let text = String::from_utf8_lossy(&answered).to_string();
    assert!(
        (200..300).contains(&parts.status.as_u16()),
        "setup: expected Amazon S3 {method} {path} to be served, got {} with {text}",
        parts.status
    );
    text
}

/// Create the function out of [`HANDLER_SOURCE`] and wait until it serves invocations.
///
/// The role it names is a role no store holds, the way
/// [`super::node_handler_sdk_answer_instants`]'s first suite names one: what is under test is
/// the form a value reaches the guest in, so the call is signed with the credential this
/// emulator hands its own handlers rather than with a role session.
async fn given_function(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": "arn:aws:iam::000000000000:role/none",
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    await_function_active(router, "setup", name).await;
}

/// Create the table `name`, wait it out to `ACTIVE` — "you can perform read and write
/// operations only on an ACTIVE table" — and answer the `CreationDateTime` a client reads for
/// it, in the epoch seconds the wire carried.
async fn given_table(router: &MultiServiceRouter, name: &str) -> f64 {
    dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(router, "DescribeTable", &json!({ "TableName": name })).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return described["Table"]["CreationDateTime"]
                .as_f64()
                .unwrap_or_else(|| {
                    panic!("setup: DescribeTable must report CreationDateTime: {described}")
                });
        }
        assert!(
            Instant::now() < deadline,
            "setup: the table {name:?} never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One instant in epoch seconds, at the MILLISECOND the two readings of it can be compared at:
/// a Python `datetime` holds microseconds, while the text a `restXml` answer carries an instant
/// in may state finer, so what is compared is the instant and not the resolution each reading
/// of it holds.
fn millis(seconds: f64) -> i64 {
    (seconds * 1_000.0).round() as i64
}

/// What one probe must find at its path.
enum Expected {
    /// The `datetime` an SDK materialises for a timestamp member, on the instant the same
    /// resource reports where a client reads it — in epoch SECONDS.
    Instant(f64),
    /// A value the model does not type as a timestamp, handed over exactly as the wire
    /// carried it.
    AsCarried(Value),
    /// The service's own refusal: its error code, and the message it worded.
    Refused(&'static str, String),
}

/// One probe: the call the handler makes, and what the value at `path` must be.
struct Probe {
    case: &'static str,
    /// The service the handler builds a client of, as `boto3.client` names it.
    service: &'static str,
    /// The client METHOD the handler calls, as the SDK documents it.
    method: &'static str,
    input: Value,
    path: Value,
    expected: Expected,
}

/// Make every probe's call in ONE invocation and assert the form each value reached the guest
/// in.
async fn probes_made(router: &MultiServiceRouter, name: &str, probes: &[Probe]) {
    let asked: Vec<Value> = probes
        .iter()
        .map(|probe| {
            json!({
                "service": probe.service,
                "method": probe.method,
                "input": probe.input,
                "path": probe.path,
            })
        })
        .collect();
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &json!({ "probes": asked }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    let signal = answered.header("x-amz-function-error");
    assert_eq!(
        signal, None,
        "expected no FunctionError, got {signal:?} with {}",
        answered.text()
    );
    let payload = answered.json();
    let reports = payload["reports"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        reports.len(),
        probes.len(),
        "expected one report per probe, got {payload}"
    );
    for (probe, report) in probes.iter().zip(&reports) {
        let case = probe.case;
        match &probe.expected {
            Expected::Instant(seconds) => {
                assert_eq!(
                    report["ok"],
                    json!(true),
                    "case {case:?}: expected the call to be served, got {report}"
                );
                assert_eq!(
                    report["type"],
                    json!("datetime"),
                    "case {case:?}: expected a datetime for a member the model types as a \
                     timestamp, got {report}"
                );
                // The SDK reports an instant in UTC, so the value carries a zone and its
                // offset is zero — which is what makes the text of it unambiguous.
                assert_eq!(
                    report["offset"].as_f64(),
                    Some(0.0),
                    "case {case:?}: expected an instant in UTC, got {report}"
                );
                let expected = millis(*seconds);
                let reported = report["epoch"].as_f64().map(millis);
                assert_eq!(
                    reported,
                    Some(expected),
                    "case {case:?}: expected the instant the service reports ({expected} ms), \
                     got {report}"
                );
                let iso = report["iso"].as_str().unwrap_or_default();
                assert!(
                    iso.contains('T') && iso.ends_with("+00:00") && !iso.starts_with("1970"),
                    "case {case:?}: expected the ISO-8601 text of this account's own clock in \
                     UTC, got {report}"
                );
                assert_eq!(
                    report["past"],
                    json!(true),
                    "case {case:?}: expected an instant that compares as already past, got \
                     {report}"
                );
                // The sub-second part the `datetime` holds is the one its own epoch value
                // carries: a fraction lost on the way into it would read as a different instant
                // from the one the member reported.
                let micros = report["epoch"]
                    .as_f64()
                    .map(|seconds| (seconds.fract().abs() * 1_000_000.0).round() as i64);
                assert_eq!(
                    report["microsecond"].as_i64(),
                    micros,
                    "case {case:?}: expected the microsecond of the instant the member reported, \
                     got {report}"
                );
            }
            Expected::AsCarried(carried) => {
                assert_eq!(
                    report["ok"],
                    json!(true),
                    "case {case:?}: expected the call to be served, got {report}"
                );
                assert_ne!(
                    report["type"],
                    json!("datetime"),
                    "case {case:?}: expected no datetime for a member the model does not type \
                     as a timestamp, got {report}"
                );
                assert_eq!(
                    report["plain"].as_f64(),
                    carried.as_f64(),
                    "case {case:?}: expected the value the wire carried ({carried}), got \
                     {report}"
                );
            }
            Expected::Refused(code, message) => {
                assert_eq!(
                    report["ok"],
                    json!(false),
                    "case {case:?}: expected the call to be refused, got {report}"
                );
                assert_eq!(
                    report["code"],
                    json!(code),
                    "case {case:?}: expected the error code {code:?}, got {report}"
                );
                assert_eq!(
                    report["message"],
                    json!(message),
                    "case {case:?}: expected the service's own message, got {report}"
                );
            }
        }
    }
}

/// RULE: a member of an answer that the called service's model types as a `timestamp` reaches a
/// `python3.x` handler as the `datetime` the SDK materialises from the epoch seconds the
/// protocol carried, in UTC and with its sub-second part intact, at whatever depth it sits and
/// whichever service answered — and a member the model types as anything else reaches it
/// exactly as the wire carried it.
#[tokio::test]
async fn an_answers_timestamp_member_reaches_a_python_handler_as_a_datetime() {
    let _stream = one_invocation_stream_at_a_time().await;
    let router = emulator();
    let table = "python-instant-items";
    let absent = "python-instant-items-absent";
    let secret = "python-instant-secret";
    let group = "/aws/lambda/python-instant-group";
    let function = "python-instant-reader";

    // Everything the probes read, created where a client creates it.
    let table_created = given_table(&router, table).await;
    aws_json(
        &router,
        "secretsmanager",
        "application/x-amz-json-1.1",
        &format!("{SECRETS_TARGET_PREFIX}.CreateSecret"),
        &json!({
            "Name": secret,
            "SecretString": "held",
            // The idempotency token every SDK client of this service fills in itself, which a
            // request made straight on the wire has to carry.
            "ClientRequestToken": "4f2a2c4e-0f1b-4f2a-9f0e-1c2d3e4f5a6b",
        }),
    )
    .await;
    let described_secret = aws_json(
        &router,
        "secretsmanager",
        "application/x-amz-json-1.1",
        &format!("{SECRETS_TARGET_PREFIX}.DescribeSecret"),
        &json!({ "SecretId": secret }),
    )
    .await;
    let secret_created = described_secret["CreatedDate"].as_f64().unwrap_or_else(|| {
        panic!("setup: DescribeSecret must report CreatedDate: {described_secret}")
    });
    aws_json(
        &router,
        "logs",
        "application/x-amz-json-1.1",
        &format!("{LOGS_TARGET_PREFIX}.CreateLogGroup"),
        &json!({ "logGroupName": group }),
    )
    .await;
    let described_groups = aws_json(
        &router,
        "logs",
        "application/x-amz-json-1.1",
        &format!("{LOGS_TARGET_PREFIX}.DescribeLogGroups"),
        &json!({ "logGroupNamePrefix": group }),
    )
    .await;
    // `logGroups[].creationTime` is a `long` of epoch MILLISECONDS, not a timestamp member:
    // the negative row below is read against the very number the wire carried.
    let group_created = described_groups["logGroups"][0]["creationTime"].clone();
    assert!(
        group_created.as_f64().is_some(),
        "setup: DescribeLogGroups must report creationTime: {described_groups}"
    );
    s3(&router, "PUT", &format!("/{BUCKET}"), &[]).await;
    s3(&router, "PUT", &format!("/{BUCKET}/{KEY}"), b"the asset").await;
    // The one object's own `LastModified`, as the `restXml` answer a client reads states it:
    // the text of an instant, which is this row's expected value.
    let listed = s3(&router, "GET", &format!("/{BUCKET}?list-type=2"), &[]).await;
    let object_modified = ::aws_smithy_types::DateTime::from_str(
        &element(&listed, "LastModified"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("setup: ListObjectsV2 must state an instant: {error}: {listed}"))
    .as_secs_f64();

    given_function(&router, function).await;

    let probes = vec![
        Probe {
            case: "a timestamp member of a nested structure (DescribeTable)",
            service: "dynamodb",
            method: "describe_table",
            input: json!({ "TableName": table }),
            path: json!(["Table", "CreationDateTime"]),
            expected: Expected::Instant(table_created),
        },
        Probe {
            case: "a count in that same answer, which is not a timestamp (ItemCount)",
            service: "dynamodb",
            method: "describe_table",
            input: json!({ "TableName": table }),
            path: json!(["Table", "ItemCount"]),
            expected: Expected::AsCarried(json!(0)),
        },
        Probe {
            case: "a top-level timestamp member of another AWS JSON version (DescribeSecret)",
            service: "secretsmanager",
            method: "describe_secret",
            input: json!({ "SecretId": secret }),
            path: json!(["CreatedDate"]),
            expected: Expected::Instant(secret_created),
        },
        Probe {
            case: "a timestamp member of a structure inside a list, on a restXml service \
                   (ListObjectsV2)",
            service: "s3",
            method: "list_objects_v2",
            input: json!({ "Bucket": BUCKET }),
            path: json!(["Contents", 0, "LastModified"]),
            expected: Expected::Instant(object_modified),
        },
        Probe {
            case: "a long carrying an epoch in milliseconds, which is not a timestamp",
            service: "logs",
            method: "describe_log_groups",
            input: json!({ "logGroupNamePrefix": group }),
            path: json!(["logGroups", 0, "creationTime"]),
            expected: Expected::AsCarried(group_created.clone()),
        },
        Probe {
            case: "a table this account does not hold, whose refusal carries no member at all",
            service: "dynamodb",
            method: "describe_table",
            input: json!({ "TableName": absent }),
            path: json!(["Table", "CreationDateTime"]),
            expected: Expected::Refused(
                ABSENT_TABLE_CODE,
                format!("Requested resource not found: Table: {absent} not found"),
            ),
        },
    ];
    probes_made(&router, function, &probes).await;
}
