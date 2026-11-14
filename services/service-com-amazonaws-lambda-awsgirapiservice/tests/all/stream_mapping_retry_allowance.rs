// SPDX-License-Identifier: CC0-1.0

//! How many times an event source mapping on a SHARD source attempts a batch its function
//! did not process, end to end over the wire — Lambda, DynamoDB, DynamoDB Streams and
//! CloudWatch on ONE engine, so the item is written where a client writes it and every
//! attempt is counted where a client counts them: the function's own `AWS/Lambda` series.
//!
//! RULE: the retries a stream mapping allows are spent on ONE batch — "(Kinesis, DynamoDB
//! Streams, Amazon MSK, and self-managed Apache Kafka) Discard records after the specified
//! number of retries. The default value is infinite (-1). When set to infinite (-1), failed
//! records are retried until the record expires"
//! (`smithy_query(query="doc", shape="CreateEventSourceMappingRequest",
//! member="MaximumRetryAttempts")`). So a mapping stating a finite count invokes its function
//! with the same batch the first attempt plus that many times, DISCARDS those records once
//! they are spent and reads on from past them — which is how the records BEHIND a batch its
//! function cannot process are still delivered — while a mapping carrying the infinite
//! default gets past them never, and keeps attempting the same batch.
//!
//! The rows differ along what the rule generalises over: the allowance the mapping states —
//! none (one attempt), several, and the infinite default the member documents.
//!
//! Nothing here waits for a span to pass: each row waits for the CONDITION it is about (the
//! result the mapping reports, the count the series holds) and fails only when a generous
//! deadline lapses.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every call below signs for, and the region every ARN carries.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Where `CreateFunction` is bound, and where `CreateEventSourceMapping` is.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";
/// See [`FUNCTIONS_URI`].
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// The `X-Amz-Target` prefix of the CloudWatch service.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `StreamViewType` every table's stream below captures
/// (`smithy_query(query="shape", shape="StreamViewType")`, DynamoDB).
const NEW_IMAGE: &str = "NEW_IMAGE";

/// The `StartingPosition` every mapping below reads from, so the records written before it
/// existed are the ones it reads (`CreateEventSourceMappingInput$StartingPosition`).
const TRIM_HORIZON: &str = "TRIM_HORIZON";

/// What `LastProcessingResult` reports for an attempt that processed the batch, and for one
/// that did not (`EventSourceMappingConfiguration$LastProcessingResult`).
const RESULT_OK: &str = "OK";
/// See [`RESULT_OK`].
const RESULT_FUNCTION_CALL_FAILED: &str = "PROBLEM: Function call failed";

/// The namespace Lambda's own metrics live in, the two series one attempt is counted on and
/// the dimension they are counted per (the AWS Lambda metric reference).
const NAMESPACE: &str = "AWS/Lambda";
/// See [`NAMESPACE`].
const INVOCATIONS: &str = "Invocations";
/// See [`NAMESPACE`].
const ERRORS: &str = "Errors";
/// See [`NAMESPACE`].
const FUNCTION_DIMENSION: &str = "FunctionName";

/// The partition key every table below is keyed on.
const KEY: &str = "pk";

/// The key of the item whose record the handler cannot process, and of the one it can.
/// Nothing in the emulator reads either: the handler reads them out of the record's keys.
const POISON: &str = "poison";
/// See [`POISON`].
const GOOD: &str = "good";

/// The `MaximumRetryAttempts` value the member documents as the infinite default: "The
/// default value is infinite (-1)."
const INFINITE_RETRIES: i64 = -1;

/// How long a wait for work the mapping has to do is given before the row is a failure. Well
/// above the table's, the function's and the mapping's own settle spans, the poller's
/// interval and the waits it puts between two attempts at one batch, so a row fails only
/// when the mapping did not do the work at all.
const BUDGET: Duration = Duration::from_secs(60);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// Lambda, DynamoDB, DynamoDB Streams and CloudWatch on one engine — the shape
/// `core/router/src/lib.rs` mounts them in.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// A SigV4 credential scope naming `signing_name` in [`REGION`]. Nothing in it is verified:
/// the harness turns request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=not-verified"
    )
}

/// One Lambda call, over its own restJson1 routes.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One RPC call against the service whose signing name is `signing_name`.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One DynamoDB call.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "dynamodb", DYNAMODB_TARGET_PREFIX, operation, body).await
}

/// One CloudWatch call.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(
        router,
        "monitoring",
        CLOUDWATCH_TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// A table keyed on [`KEY`] whose stream captures [`NEW_IMAGE`], waited out to `ACTIVE`;
/// answers the table's `LatestStreamArn`.
async fn given_table(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "AttributeDefinitions": [{ "AttributeName": KEY, "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": KEY, "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": { "StreamEnabled": true, "StreamViewType": NEW_IMAGE },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateTable answered {status} {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = dynamodb(router, "DescribeTable", &json!({ "TableName": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeTable answered {status} {read}"
        );
        if read["Table"]["TableStatus"] == json!("ACTIVE") {
            return read["Table"]["LatestStreamArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no LatestStreamArn in {read}"))
                .to_string();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Write the item `key` names into `table`.
async fn write_item(router: &MultiServiceRouter, case: &str, table: &str, key: &str) {
    let (status, written) = dynamodb(
        router,
        "PutItem",
        &json!({ "TableName": table, "Item": { KEY: { "S": key } } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutItem answered {status} {written}"
    );
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

/// The handler every row runs: it RAISES on the record of the poisoned item and returns on
/// every other one, so what each attempt comes to is decided by the record it was handed.
fn failing_handler() -> String {
    format!(
        r#"
KEY = "{KEY}"
POISON = "{POISON}"


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no record")
    for record in records:
        key = record["dynamodb"]["Keys"][KEY]["S"]
        if key == POISON:
            raise RuntimeError("the handler cannot process " + key)
    return {{"handled": len(records)}}
"#
    )
}

/// A function running [`failing_handler`], waited out to `Active`.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&failing_handler())),
            },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateFunction answered {status} {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetFunctionConfiguration answered {status} {read}"
        );
        if read["State"] == json!("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function never became Active: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Create the mapping between `function` and `stream_arn`, reading one record per batch and
/// allowing `retries` retries of a batch; answers its UUID.
async fn given_mapping(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    stream_arn: &str,
    retries: i64,
) -> String {
    let (status, created) = lambda(
        router,
        "POST",
        MAPPINGS_URI,
        &json!({
            "FunctionName": function,
            "EventSourceArn": stream_arn,
            "StartingPosition": TRIM_HORIZON,
            "BatchSize": 1,
            "MaximumRetryAttempts": retries,
            "Enabled": true,
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateEventSourceMapping answered {status} {created}"
    );
    assert_eq!(
        created["MaximumRetryAttempts"],
        json!(retries),
        "case {case:?}: the mapping must report the allowance it was created with, got {created}"
    );
    created["UUID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {created}"))
        .to_string()
}

/// What `GetEventSourceMapping` reports for one mapping.
async fn get_mapping(router: &MultiServiceRouter, case: &str, uuid: &str) -> Value {
    let (status, read) =
        lambda(router, "GET", &format!("{MAPPINGS_URI}/{uuid}"), &Value::Null).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetEventSourceMapping answered {status} {read}"
    );
    read
}

/// The sum one `AWS/Lambda` series holds for `function`, over a window around now.
async fn series_sum(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    function: &str,
) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let (status, stats) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{ "Name": FUNCTION_DIMENSION, "Value": function }],
            "StartTime": now - 600.0,
            "EndTime": now + 600.0,
            "Period": 60,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetMetricStatistics answered {status} {stats}"
    );
    stats["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// Wait until the `Errors` series of `function` holds at least `wanted`, answering the sum it
/// came to.
async fn errors_reaching(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    wanted: f64,
) -> f64 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let errors = series_sum(router, case, ERRORS, function).await;
        if errors >= wanted {
            return errors;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the function's {ERRORS} series to reach {wanted}, got \
             {errors}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until the mapping reports `expected` as its `LastProcessingResult`.
async fn result_becomes(router: &MultiServiceRouter, case: &str, uuid: &str, expected: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = get_mapping(router, case, uuid).await;
        let reported = read["LastProcessingResult"].as_str().unwrap_or_default();
        if reported == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the mapping to come to report {expected:?} as its \
             LastProcessingResult, got {reported:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the allowance the mapping states, and how many attempts that is.
struct Row {
    case: &'static str,
    /// `MaximumRetryAttempts`, as the request states it.
    retries: i64,
    /// How many attempts at the poisoned batch that allows — the first one plus the retries —
    /// or `None` for the infinite default, which allows every attempt.
    attempts: Option<f64>,
}

/// RULE: a stream mapping attempts a batch its function did not process the first time plus
/// as many times as its `MaximumRetryAttempts` allows, then discards those records and reads
/// on from past them — so the record behind them is delivered. A mapping carrying the
/// infinite default discards nothing, so it never gets past them.
#[tokio::test]
async fn the_retries_a_stream_mapping_allows_are_spent_on_one_batch() {
    let rows = [
        Row {
            case: "no retry at all, so one attempt",
            retries: 0,
            attempts: Some(1.0),
        },
        Row {
            case: "two retries, so three attempts",
            retries: 2,
            attempts: Some(3.0),
        },
        Row {
            case: "the infinite default, so every attempt",
            retries: INFINITE_RETRIES,
            attempts: None,
        },
    ];
    for row in rows {
        let Row {
            case,
            retries,
            attempts,
        } = row;
        let router = emulator();
        let table = format!("retries-{retries}");
        let function = format!("retries-{retries}-handler");
        let stream_arn = given_table(&router, case, &table).await;
        // The poisoned item FIRST, so the record the function cannot process is the one at
        // the head of the shard and the good record sits behind it.
        write_item(&router, case, &table, POISON).await;
        write_item(&router, case, &table, GOOD).await;
        given_function(&router, case, &function).await;
        let uuid = given_mapping(&router, case, &function, &stream_arn, retries).await;

        match attempts {
            Some(allowed) => {
                // The records behind the discarded batch are delivered, which is what the
                // mapping comes to report as its last attempt.
                result_becomes(&router, case, &uuid, RESULT_OK).await;
                let errors = series_sum(&router, case, ERRORS, &function).await;
                assert_eq!(
                    errors, allowed,
                    "case {case:?}: expected exactly {allowed} failed attempt(s) at the \
                     poisoned batch, got {errors}"
                );
                // One invocation per failed attempt, plus the one that delivered the record
                // behind them.
                let invocations = series_sum(&router, case, INVOCATIONS, &function).await;
                assert_eq!(
                    invocations,
                    allowed + 1.0,
                    "case {case:?}: expected {} invocation(s) in all, got {invocations}",
                    allowed + 1.0
                );
            }
            None => {
                // Two failed attempts at the same batch show the retrying; the point of the
                // row is that NOTHING is ever checkpointed past it, so the record behind it
                // is never delivered and no invocation of this function ever succeeded.
                let errors = errors_reaching(&router, case, &function, 2.0).await;
                let read = get_mapping(&router, case, &uuid).await;
                assert_eq!(
                    read["LastProcessingResult"],
                    json!(RESULT_FUNCTION_CALL_FAILED),
                    "case {case:?}: a mapping that discards nothing must keep reporting the \
                     failed attempt, got {read}"
                );
                let invocations = series_sum(&router, case, INVOCATIONS, &function).await;
                assert_eq!(
                    invocations, errors,
                    "case {case:?}: every invocation must be one of the {errors} failed \
                     attempts at the poisoned batch, got {invocations}"
                );
            }
        }
    }
}
