// SPDX-License-Identifier: CC0-1.0

//! The shape of a function an object event runs: Lambda, Amazon S3, Amazon Rekognition and
//! Amazon DynamoDB on ONE engine, and a `python3.x` handler that asks the image analysis
//! service what an object holds and records the answer in a table.
//!
//! RULE: a managed runtime ships the AWS SDK on the handler's import path, so a client the
//! handler builds for ANY service this emulator serves resolves and its call reaches that
//! service AS MADE — the output members that service measured for a success, and that
//! service's own error code and message for a refusal — and what the handler then records
//! with a second service is readable through that service's own API. Nothing the handler
//! reports is this runtime's: every label asserted below is compared with what the analysing
//! service answers for the same image when a client calls it directly.
//!
//! The rows differ along what the rule generalises over: where the image came from (an object
//! in the store, and bytes the event carried inline), the image's format (PNG and JPEG — "the
//! image must be either a PNG or JPEG formatted file"), whether the request narrows the answer
//! (`MaxLabels`), and whether the image can be read at all, which is the refusal half of the
//! rule.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use image::{ImageFormat, Rgb, RgbImage};
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
// The ONE writer of a deployment archive in this target, rather than a second copy of it.
use crate::synchronous_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How long this suite waits for a status the clock owes, and how often it looks again.
const BUDGET: Duration = Duration::from_secs(60);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The bucket the images are staged in, the table the handler records into, and that table's
/// one key attribute — the name of the image a row analysed.
const BUCKET: &str = "handler-image-analysis";
const TABLE: &str = "handler-image-analysis-labels";
const KEY_ATTRIBUTE: &str = "image_name";

/// The service shape an awsJson `X-Amz-Target` names an Amazon Rekognition operation under
/// (`com.amazonaws.rekognition#RekognitionService`), and the signing name that routes a call
/// to it (`sigv4Name` `rekognition`) — both read with
/// `smithy_query(query="service_info")` against that service's own model.
const REKOGNITION_TARGET_PREFIX: &str = "RekognitionService";
const REKOGNITION_SIGNING_NAME: &str = "rekognition";

/// Lambda, Amazon S3, Amazon Rekognition and Amazon DynamoDB over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own SDK calls are dispatched back into
/// this same router, so the image it analyses is the object this suite staged and the item it
/// writes is the item this suite reads.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_rekognition_rekognitionservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
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

/// One AWS JSON call on the wire: the status and the parsed body.
async fn aws_json(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    json_version: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", target)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Amazon DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer
/// 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = aws_json(
        router,
        "dynamodb",
        &format!("DynamoDB_20120810.{operation}"),
        "1.0",
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Amazon Rekognition call (`awsJson1_1`), answered as it came so a case can read a
/// refusal as well as an answer.
async fn rekognition(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    aws_json(
        router,
        REKOGNITION_SIGNING_NAME,
        &format!("{REKOGNITION_TARGET_PREFIX}.{operation}"),
        "1.1",
        body,
    )
    .await
}

/// One request to Amazon S3, whose protocol is a method and a path rather than a document.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
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
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// The handler this suite runs: for every row it is handed it asks the image analysis service
/// what the image holds and writes the names it answered into the table, keyed by the image's
/// own name — and reports a refusal as the service's own code and message rather than
/// recording anything.
const HANDLER_SOURCE: &str = r#"
import base64

import boto3
import botocore.exceptions

# Built at INITIALIZATION, as a handler that analyses every object it is handed does: a
# service token the runtime's SDK does not answer to cannot even be a client here.
rekognition = boto3.client("rekognition")
dynamodb = boto3.client("dynamodb")


def lambda_handler(event, context):
    reported = {}
    for row in event["rows"]:
        if row.get("key") is not None:
            image = {"S3Object": {"Bucket": event["bucket"], "Name": row["key"]}}
        else:
            image = {"Bytes": base64.b64decode(row["bytes"])}
        request = {"Image": image}
        if row.get("max_labels") is not None:
            request["MaxLabels"] = row["max_labels"]
        try:
            answered = rekognition.detect_labels(**request)
        except botocore.exceptions.ClientError as error:
            reported[row["case"]] = {
                "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"],
            }
            continue
        names = [label["Name"] for label in answered["Labels"]]
        dynamodb.put_item(
            TableName=event["table"],
            Item={
                "image_name": {"S": row["name"]},
                "labels": {"L": [{"S": name} for name in names]},
                "model": {"S": answered.get("LabelModelVersion") or ""},
            },
        )
        reported[row["case"]] = {
            "labels": names,
            "model": answered.get("LabelModelVersion"),
        }
    return reported
"#;

/// Create the function and wait out the provisioning window its create documents.
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// The table the handler records into, waited out to `ACTIVE` — "you can perform read and
/// write operations only on an ACTIVE table".
async fn given_table(router: &MultiServiceRouter, case: &str) {
    dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = dynamodb(router, case, "DescribeTable", &json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// An image of `width` by `height` pixels: the left `split` columns in `left` and the rest in
/// `right`, so one row can carry a single colour (`split` covering the frame) and another two
/// of them.
fn frame(width: u32, height: u32, split: u32, left: Rgb<u8>, right: Rgb<u8>) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _y| if x < split { left } else { right })
}

/// One image's bytes in `format`, encoded by the image library rather than written out here.
fn encoded(image: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut bytes, format)
        .expect("the frame encodes");
    bytes.into_inner()
}

/// Where the image a row analyses came from.
enum Source {
    /// An object in the store, named by its key — the shape an object event hands a handler.
    Object(&'static str),
    /// Bytes the event itself carried, which the handler passes inline.
    Inline,
}

/// One row: the case, the name the record is keyed by, where the image came from, the bytes it
/// carries, how many labels the request asks for, and whether the image can be read at all.
struct Row {
    case: &'static str,
    name: &'static str,
    source: Source,
    bytes: Vec<u8>,
    max_labels: Option<i64>,
    /// The error code the analysing service must answer for an image it cannot read, or `None`
    /// for a row whose image it reads.
    refused_as: Option<&'static str>,
}

/// The rows, one per thing the rule generalises over.
fn rows() -> Vec<Row> {
    let single = frame(64, 48, 64, Rgb([32, 72, 180]), Rgb([32, 72, 180]));
    let halves = frame(80, 60, 40, Rgb([200, 40, 40]), Rgb([240, 240, 240]));
    let png = encoded(&single, ImageFormat::Png);
    vec![
        Row {
            case: "a PNG object in the store",
            name: "alpha.png",
            source: Source::Object("incoming/alpha.png"),
            bytes: png.clone(),
            max_labels: None,
            refused_as: None,
        },
        Row {
            case: "the same PNG handed over as inline bytes",
            name: "alpha-inline.png",
            source: Source::Inline,
            bytes: png.clone(),
            max_labels: None,
            refused_as: None,
        },
        Row {
            case: "a JPEG object in the store",
            name: "beta.jpg",
            source: Source::Object("incoming/beta.jpg"),
            bytes: encoded(&halves, ImageFormat::Jpeg),
            max_labels: None,
            refused_as: None,
        },
        Row {
            case: "a PNG object narrowed to one label",
            name: "gamma.png",
            source: Source::Object("incoming/gamma.png"),
            bytes: encoded(&halves, ImageFormat::Png),
            max_labels: Some(1),
            refused_as: None,
        },
        Row {
            // "The provided image format is not supported" — the modelled refusal for bytes
            // that are no image, which is what the handler must be handed rather than an
            // answer of this runtime's.
            case: "bytes that are no image at all",
            name: "delta.png",
            source: Source::Inline,
            bytes: png[..png.len() / 3].to_vec(),
            max_labels: None,
            refused_as: Some("InvalidImageFormatException"),
        },
    ]
}

/// The `Image` member a row's request carries, as a client builds it.
fn image_member(row: &Row) -> Value {
    match row.source {
        Source::Object(key) => json!({"S3Object": {"Bucket": BUCKET, "Name": key}}),
        Source::Inline => {
            json!({"Bytes": base64::engine::general_purpose::STANDARD.encode(&row.bytes)})
        }
    }
}

/// The label names and the model version a DIRECT `DetectLabels` call answers for one row's
/// image, which is what the handler's own report is compared with.
async fn directly_detected(
    router: &MultiServiceRouter,
    row: &Row,
) -> (u16, Vec<String>, Option<String>, Value) {
    let mut request = json!({"Image": image_member(row)});
    if let Some(max_labels) = row.max_labels {
        request["MaxLabels"] = json!(max_labels);
    }
    let (status, answered) = rekognition(router, "DetectLabels", &request).await;
    let names = answered["Labels"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    let model = answered["LabelModelVersion"]
        .as_str()
        .map(str::to_string);
    (status, names, model, answered)
}

/// The item the table holds under `name`, or `None` for a name nothing was written under.
async fn held_item(router: &MultiServiceRouter, case: &str, name: &str) -> Option<Value> {
    let read = dynamodb(
        router,
        case,
        "GetItem",
        &json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": name}}}),
    )
    .await;
    read.get("Item").filter(|item| !item.is_null()).cloned()
}

/// Stage the bucket and every row's image in it, which is what an object event hands a
/// handler the location of.
async fn given_staged_images(router: &MultiServiceRouter, case: &str, rows: &[Row]) {
    let (created, body) = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created, 200,
        "case {case:?}: expected CreateBucket to answer 200, got {created}: {body}"
    );
    for row in rows {
        if let Source::Object(key) = row.source {
            let (stored, body) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), &row.bytes).await;
            assert_eq!(
                stored, 200,
                "case {:?}: expected PutObject({key:?}) to answer 200, got {stored}: {body}",
                row.case
            );
        }
    }
}

/// Invoke the function over every row at once, and answer what the handler reported.
async fn reported_for_every_row(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    rows: &[Row],
) -> Value {
    let asked: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut row_json = json!({ "case": row.case, "name": row.name });
            match row.source {
                Source::Object(key) => row_json["key"] = json!(key),
                Source::Inline => {
                    row_json["bytes"] =
                        json!(base64::engine::general_purpose::STANDARD.encode(&row.bytes));
                }
            }
            if let Some(max_labels) = row.max_labels {
                row_json["max_labels"] = json!(max_labels);
            }
            row_json
        })
        .collect();
    let (status, headers, reported) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        json!({ "bucket": BUCKET, "table": TABLE, "rows": asked })
            .to_string()
            .as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({reported})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got the function error {signal:?} \
         with {reported}"
    );
    reported
}

/// One row's assertions: what the handler reported against what a DIRECT call measures for the
/// same image, and what the table holds afterwards. The labels the service measured, for the
/// rows it measured any.
async fn row_reports_what_the_service_answered(
    router: &MultiServiceRouter,
    row: &Row,
    reported: &Value,
) -> Option<Vec<String>> {
    let case = row.case;
    let held = &reported[case];
    let (status, direct, model, answered) = directly_detected(router, row).await;
    match row.refused_as {
        Some(code) => {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the analysing service to refuse the direct call too, \
                 got {status} with {answered}"
            );
            let reported_code = held["code"].as_str().unwrap_or_default();
            assert!(
                reported_code.ends_with(code),
                "case {case:?}: expected the handler to be handed the service's own {code:?}, \
                 got {held}"
            );
            assert!(
                held["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: expected the service's own message, got {held}"
            );
            assert_eq!(
                held_item(router, case, row.name).await,
                None,
                "case {case:?}: a refused analysis records nothing, so {:?} must hold no item",
                row.name
            );
            return None;
        }
        None => assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from a direct DetectLabels, got {status} with \
             {answered}"
        ),
    }

    assert!(
        !direct.is_empty(),
        "case {case:?}: expected the analysing service to measure at least one label for the \
         image, got {answered}"
    );
    let handler_labels: Vec<String> = held["labels"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        handler_labels, direct,
        "case {case:?}: expected the handler to report the labels the service measured \
         ({direct:?}), got {held}"
    );
    assert_eq!(
        held["model"].as_str().map(str::to_string),
        model,
        "case {case:?}: expected the model version the service reported ({model:?}), got {held}"
    );
    // "The maximum number of labels you want the service to return in the response"
    // (`DetectLabelsRequest$MaxLabels`), so the narrowed answer carries no more than that.
    if let Some(max_labels) = row.max_labels {
        assert!(
            i64::try_from(handler_labels.len()).unwrap_or(i64::MAX) <= max_labels,
            "case {case:?}: expected the answer narrowed to at most {max_labels} label(s), got \
             {handler_labels:?}"
        );
    }

    // What the TABLE holds: the record the handler wrote, read back through DynamoDB's own
    // API, keyed by the image's name.
    let item = held_item(router, case, row.name).await.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the handler's record of {:?}",
            row.name
        )
    });
    assert_eq!(
        item[KEY_ATTRIBUTE],
        json!({"S": row.name}),
        "case {case:?}: expected the record keyed by the image's name, got {item}"
    );
    let recorded: Vec<String> = item["labels"]["L"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| value["S"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        recorded, direct,
        "case {case:?}: expected the stored labels to be the measured ones ({direct:?}), got \
         {item}"
    );
    assert_eq!(
        item["model"],
        json!({"S": model.clone().unwrap_or_default()}),
        "case {case:?}: expected the stored model version to be the reported one, got {item}"
    );
    Some(direct)
}

/// Every row, in ONE invocation: the handler's client for the analysing service resolves, its
/// call is answered by that service itself — the very labels a direct call measures for the
/// same image — and what the handler recorded is read back through the table's own API, while
/// an image that cannot be read reaches the handler as that service's own modelled refusal and
/// records nothing.
///
/// Each phase is awaited through its own heap-allocated future, so the test's own frame stays
/// small whatever the request paths beneath it cost.
#[tokio::test]
async fn a_handler_records_what_the_image_analysis_service_answered() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "a handler analysing the objects it was handed";
    let router = router();
    let rows = rows();
    let function = "image-analysis-recorder";

    Box::pin(given_staged_images(&router, case, &rows)).await;
    Box::pin(given_table(&router, case)).await;
    Box::pin(given_function(&router, case, function)).await;
    let reported = Box::pin(reported_for_every_row(&router, case, function, &rows)).await;

    let mut labels_by_case: Vec<(&str, Vec<String>)> = Vec::new();
    for row in &rows {
        if let Some(labels) =
            Box::pin(row_reports_what_the_service_answered(&router, row, &reported)).await
        {
            labels_by_case.push((row.case, labels));
        }
    }

    // The other half of the rule, over the two rows carrying ONE image: an image answers alike
    // whether it travelled as an object in the store or as bytes beside the request.
    let object_labels = labels_by_case
        .iter()
        .find(|(case, _)| *case == "a PNG object in the store")
        .map(|(_, labels)| labels.clone())
        .unwrap_or_default();
    let inline_labels = labels_by_case
        .iter()
        .find(|(case, _)| *case == "the same PNG handed over as inline bytes")
        .map(|(_, labels)| labels.clone())
        .unwrap_or_default();
    assert_eq!(
        object_labels, inline_labels,
        "case {case:?}: one image answers alike however it was submitted, got {object_labels:?} \
         and {inline_labels:?}"
    );
}
