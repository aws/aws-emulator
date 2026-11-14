// SPDX-License-Identifier: CC0-1.0

//! The Amazon Rekognition call a RUNNING handler makes, bound end to end over the wire:
//! Lambda, Amazon Rekognition, Amazon S3 and Amazon DynamoDB on ONE engine, a function whose
//! code asks the image analysis service what a staged frame carries, and the record it writes
//! read back where a client reads it.
//!
//! RULE: a managed runtime ships the AWS SDK on the handler's import path, so a client the
//! handler builds for ANY service this emulator serves whose protocol the runtime's call seam
//! binds resolves, and its call reaches that service AS MADE — the output members that service
//! measured for a success, and that service's own modelled error for a refusal — and what the
//! handler then records with a second service is readable through that service's own API.
//! Nothing the handler reports is this runtime's: every frame below is DRAWN by this suite from
//! known colours, so each assertion compares the service's answer with what the test put in the
//! image.
//!
//! The rows differ along what the rule generalises over: which operation is called (two reads
//! of an image, a write that mints a resource, the list that reports it), which analysis a read
//! asks for (`GENERAL_LABELS` by default, `IMAGE_PROPERTIES` by name), where the image came
//! from (an object in the store, and the bytes the event carried inline), what the frame
//! carries (one colour, two colours), the format it was staged in (PNG, JPEG — "the image must
//! be either a PNG or JPEG formatted file"), whether the request narrows the answer
//! (`MaxLabels`), and whether the call is served or refused — and, where it is refused, which of
//! the two unreadable-image classes it falls into (an address the store does not serve, bytes
//! that are no image at all). Both managed runtimes that bundle an SDK are driven.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The ONE writer of a deployment archive in this target, rather than a second copy of it.
use crate::node_invoke_run::package;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The managed runtimes that bundle an SDK, one per language this suite drives.
const NODE_RUNTIME: &str = "nodejs20.x";
const PYTHON_RUNTIME: &str = "python3.12";

/// How long a status the clock owes is waited for before the wait itself is the failure, and
/// how long this suite leaves between two readings of it. Every wait looks again for the
/// CONDITION rather than sleeping for a span.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The bucket the frames are staged in, and the keys each is staged under.
const BUCKET: &str = "vision-landing";
const FLAT_PNG_KEY: &str = "uploads/one-colour.png";
const FLAT_JPEG_KEY: &str = "uploads/one-colour.jpg";
const TWO_COLOUR_KEY: &str = "uploads/two-colours.png";
const ABSENT_KEY: &str = "uploads/never-written.png";

/// The table the handler records what it measured in.
const TABLE: &str = "vision-labels";

/// The colours the frames are drawn from, and the label name this emulator's Rekognition
/// reports each as — one of the twelve simplified colour names, capitalised
/// (`smithy_query(query="doc",shape="DominantColor",member="SimplifiedColor")`: "One of 12
/// simplified color names").
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const RED_LABEL: &str = "Red";
const BLUE_LABEL: &str = "Blue";

/// The composition label a frame that is essentially ONE colour also carries.
const MONOCHROME_LABEL: &str = "Monochrome";

/// The version of the label detection model the service answers with, which is the member's
/// own documentation: "version number of the label detection model that was used to detect
/// labels".
const LABEL_MODEL_VERSION: &str = "3.0";

/// The frames' size in pixels. Small enough to encode and analyse quickly, large enough that
/// each half of the two-colour frame is unambiguous.
const WIDTH: u32 = 120;
const HEIGHT: u32 = 80;

/// The modelled refusal of an `S3Object` the store does not serve, and the message that error
/// class carries (`smithy_query(query="shape",shape="InvalidS3ObjectException")`: "Amazon
/// Rekognition is unable to access the S3 object specified in the request").
const UNREADABLE_OBJECT: &str = "InvalidS3ObjectException";
const UNREADABLE_OBJECT_MESSAGE: &str =
    "Unable to get object metadata from S3. Check object key, region and/or access permissions.";

/// The OTHER refusal class: an address the store serves whose BYTES are no image this service
/// reads (`smithy_query(query="shape",shape="InvalidImageFormatException")`: "The provided image
/// format is not supported").
const INVALID_FORMAT: &str = "InvalidImageFormatException";
const INVALID_FORMAT_MESSAGE: &str = "Request has invalid image format";

/// The collection the write probe mints, and the member it is reported back under.
const COLLECTION: &str = "vision-faces";

// ---------------------------------------------------------------------------
// The frames this suite draws
// ---------------------------------------------------------------------------

/// A frame drawn entirely in one colour.
fn flat(colour: [u8; 3]) -> RgbImage {
    RgbImage::from_pixel(WIDTH, HEIGHT, Rgb(colour))
}

/// A frame whose left half carries `left` and whose right half carries `right`.
fn halves(left: [u8; 3], right: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(WIDTH, HEIGHT, |x, _| {
        Rgb(if x < WIDTH / 2 { left } else { right })
    })
}

/// A frame as the bytes of one of the two formats the service reads: "the image must be either
/// a PNG or JPEG formatted file".
fn encoded(frame: &RgbImage, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(frame.clone())
        .write_to(&mut bytes, format)
        .expect("the drawn frame encodes");
    bytes.into_inner()
}

// ---------------------------------------------------------------------------
// The emulator and the wires this suite dispatches on
// ---------------------------------------------------------------------------

/// Lambda, Amazon Rekognition, Amazon S3 and Amazon DynamoDB over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's SDK call is dispatched back into this
/// same router, so the object it analyses and the table it writes are the ones this suite
/// stages and reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_rekognition_rekognitionservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Amazon S3 call made as a CLIENT makes it — path-style, signed for `s3` — answering its
/// status and its body bytes.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, bytes::Bytes) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body())
}

/// One Amazon DynamoDB call on the wire (`awsJson1_0`), which must answer 200.
async fn dynamodb(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("authorization", authorization("dynamodb"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
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
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {} with {parsed}",
        parts.status
    );
    parsed
}

/// One Amazon Rekognition call a CLIENT makes directly (`awsJson1_1`), as its status and parsed
/// body: what the handler read back is compared with what this service answers for the same
/// image when a client asks it.
async fn rekognition(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("rekognition.{REGION}.amazonaws.com"))
        .header("authorization", authorization("rekognition"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("RekognitionService.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered).unwrap_or(Value::Null)
    };
    (parts.status.as_u16(), parsed)
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
    let (parts, answered) = response.into_parts();
    let parsed: Value = if answered.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&answered)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&answered).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// Create a function whose `Runtime` is `runtime`, whose package holds `source` as `entry` and
/// whose `Handler` is `handler`, then wait for the state an invocation needs.
async fn create_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    entry: &str,
    handler: &str,
    source: &str,
    runtime: &str,
) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": runtime,
            "Handler": handler,
            "Timeout": 30,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[(entry, source)])),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    // "The function is being created" until it is Active, which is the state an invocation
    // needs: the wait is for that CONDITION, read through the function's own configuration.
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, _headers, configuration) = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &[],
        )
        .await;
        if status == 200 && configuration["State"] == json!("Active") {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the function never became Active: {status} ({configuration})"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// `Invoke` the function synchronously with `event`, answering the `FunctionError` signal the
/// response carried and the payload the run produced.
async fn invoke(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    event: &Value,
) -> (Option<String>, Value) {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (signal, payload)
}

/// Stage the bucket, the three frames and the table every row stands on, and answer the base64
/// of the one frame a row submits INLINE.
async fn staged(router: &MultiServiceRouter, case: &str) -> String {
    let (status, answered) = s3(router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected CreateBucket to answer 200, got {status} ({})",
        String::from_utf8_lossy(&answered)
    );
    let two_colour = encoded(&halves(RED, BLUE), ImageFormat::Png);
    let objects: [(&str, Vec<u8>); 3] = [
        (FLAT_PNG_KEY, encoded(&flat(RED), ImageFormat::Png)),
        (FLAT_JPEG_KEY, encoded(&flat(RED), ImageFormat::Jpeg)),
        (TWO_COLOUR_KEY, two_colour.clone()),
    ];
    for (key, bytes) in &objects {
        let (status, answered) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), bytes).await;
        assert_eq!(
            status,
            200,
            "case {case:?}: expected PutObject({key:?}) to answer 200, got {status} ({})",
            String::from_utf8_lossy(&answered)
        );
    }
    dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{ "AttributeName": "image_name", "KeyType": "HASH" }],
            "AttributeDefinitions": [
                { "AttributeName": "image_name", "AttributeType": "S" },
            ],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    // "You can perform read and write operations only on an ACTIVE table": the wait is for
    // that CONDITION, read through the table's own description.
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = dynamodb(
            router,
            case,
            "DescribeTable",
            &json!({ "TableName": TABLE }),
        )
        .await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    base64::engine::general_purpose::STANDARD.encode(&two_colour)
}

/// The label names an answered analysis reported, in the order the service reported them.
fn label_names(answer: &Value) -> Vec<String> {
    answer["Labels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|label| label["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The handlers
// ---------------------------------------------------------------------------

/// The node handler: it builds the image analysis client from the bundled
/// `@aws-sdk/client-rekognition`, sends each probe's command, and reports what the service
/// answered or refused with — then does what the pipeline this suite stands for does, recording
/// the labels of the object the event named in the table, through the bundled DynamoDB client.
const NODE_HANDLER: &str = r#"
const {
    RekognitionClient,
    DetectLabelsCommand,
    DetectFacesCommand,
    CreateCollectionCommand,
    ListCollectionsCommand,
    InvalidS3ObjectException,
} = require('@aws-sdk/client-rekognition');
const { DynamoDBClient, PutItemCommand } = require('@aws-sdk/client-dynamodb');

const COMMANDS = {
    DetectLabels: DetectLabelsCommand,
    DetectFaces: DetectFacesCommand,
    CreateCollection: CreateCollectionCommand,
    ListCollections: ListCollectionsCommand,
};

exports.handler = async (event) => {
    const vision = new RekognitionClient({});
    const report = {};
    for (const probe of event.probes) {
        const Command = COMMANDS[probe.operation];
        const held = { bound: typeof Command === 'function' };
        if (held.bound) {
            try {
                held.answer = await vision.send(new Command(probe.input));
                held.outcome = 'answered';
            } catch (error) {
                held.outcome = 'refused';
                held.name = error.name;
                held.message = error.message;
                held.status = error.$metadata === undefined || error.$metadata === null
                    ? null
                    : error.$metadata.httpStatusCode;
                held.unreadable_object = error instanceof InvalidS3ObjectException;
            }
        }
        report[probe.case] = held;
    }

    const measured = await vision.send(new DetectLabelsCommand({
        Image: { S3Object: { Bucket: event.bucket, Name: event.key } },
    }));
    const names = (measured.Labels || []).map(function (label) { return label.Name; });
    await new DynamoDBClient({}).send(new PutItemCommand({
        TableName: event.table,
        Item: {
            image_name: { S: event.key },
            labels: { SS: names },
            model_version: { S: measured.LabelModelVersion },
        },
    }));
    return { report: report, recorded: names };
};
"#;

/// The python handler: the same calls through the SDK that runtime bundles — the analysis of the
/// staged object, the analysis of bytes the event carried inline, a refusal of an object the
/// store does not serve, and the record it writes.
const PYTHON_HANDLER: &str = r#"
import base64

import boto3
import botocore.exceptions


def lambda_handler(event, context):
    vision = boto3.client("rekognition")
    measured = vision.detect_labels(
        Image={"S3Object": {"Bucket": event["bucket"], "Name": event["key"]}}
    )
    names = [label["Name"] for label in measured.get("Labels", [])]
    inline = vision.detect_labels(
        Image={"Bytes": base64.b64decode(event["inline"])}, MaxLabels=1
    )
    refused = None
    try:
        vision.detect_labels(
            Image={"S3Object": {"Bucket": event["bucket"], "Name": event["absent"]}}
        )
    except botocore.exceptions.ClientError as error:
        refused = {
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
        }
    boto3.client("dynamodb").put_item(
        TableName=event["table"],
        Item={
            "image_name": {"S": event["key"]},
            "labels": {"SS": names},
            "model_version": {"S": measured["LabelModelVersion"]},
        },
    )
    return {
        "recorded": names,
        "model_version": measured["LabelModelVersion"],
        "inline": [label["Name"] for label in inline.get("Labels", [])],
        "refused": refused,
    }
"#;

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One probe: the operation the handler calls, the input it carries, and whether the service
/// serves it.
struct Probe {
    case: &'static str,
    operation: &'static str,
    input: fn() -> Value,
    served: bool,
}

/// The probes, one per thing the rule generalises over: a read of a one-colour frame staged as
/// PNG, the same frame staged as JPEG, a two-colour frame, the same read narrowed by
/// `MaxLabels`, a read asking for the other analysis, another read operation of the same
/// service, a write that mints a resource, the list that reports it, a read of an address the
/// store does not serve, and a read of bytes that are no image at all — the two refusal classes
/// an unreadable image falls into.
fn probes() -> Vec<Probe> {
    vec![
        Probe {
            case: "one colour, staged as PNG",
            operation: "DetectLabels",
            input: || json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": FLAT_PNG_KEY } } }),
            served: true,
        },
        Probe {
            case: "the same frame staged in the other format the service reads",
            operation: "DetectLabels",
            input: || {
                json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": FLAT_JPEG_KEY } } })
            },
            served: true,
        },
        Probe {
            case: "two colours, neither of them the whole frame",
            operation: "DetectLabels",
            input: || {
                json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": TWO_COLOUR_KEY } } })
            },
            served: true,
        },
        Probe {
            case: "the same frame with the answer narrowed to one label",
            operation: "DetectLabels",
            input: || {
                json!({
                    "Image": { "S3Object": { "Bucket": BUCKET, "Name": TWO_COLOUR_KEY } },
                    "MaxLabels": 1,
                })
            },
            served: true,
        },
        Probe {
            case: "the other analysis of the same image, asked for by name",
            operation: "DetectLabels",
            input: || {
                json!({
                    "Image": { "S3Object": { "Bucket": BUCKET, "Name": FLAT_PNG_KEY } },
                    "Features": ["IMAGE_PROPERTIES"],
                })
            },
            served: true,
        },
        Probe {
            case: "another read operation of the same service",
            operation: "DetectFaces",
            input: || json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": FLAT_PNG_KEY } } }),
            served: true,
        },
        Probe {
            case: "a write that mints a resource",
            operation: "CreateCollection",
            input: || json!({ "CollectionId": COLLECTION }),
            served: true,
        },
        Probe {
            case: "the list that reports what the write minted",
            operation: "ListCollections",
            input: || json!({}),
            served: true,
        },
        Probe {
            case: "an image at an address the store does not serve",
            operation: "DetectLabels",
            input: || json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": ABSENT_KEY } } }),
            served: false,
        },
        Probe {
            case: "bytes that are no image at all",
            operation: "DetectLabels",
            input: || {
                json!({
                    "Image": {
                        "Bytes": base64::engine::general_purpose::STANDARD
                            .encode(b"not an image at all"),
                    },
                })
            },
            served: false,
        },
    ]
}

/// Every probe: the node runtime's bundled SDK dispatches the call to this emulator's own
/// Amazon Rekognition, which answers the operation's own output members for a frame it could
/// read and its own modelled refusal for an address the store does not serve — and the labels
/// the handler then records are read back where a client reads them.
#[tokio::test]
async fn a_node_handler_analyses_an_image_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a node handler calling Amazon Rekognition through the runtime's SDK";
    let router = emulator();
    staged(&router, case).await;

    let caller = "image-analysis-node-caller";
    create_function(
        &router,
        case,
        caller,
        "index.js",
        "index.handler",
        NODE_HANDLER,
        NODE_RUNTIME,
    )
    .await;

    let probes = probes();
    let asked: Vec<Value> = probes
        .iter()
        .map(|probe| {
            json!({
                "case": probe.case,
                "operation": probe.operation,
                "input": (probe.input)(),
            })
        })
        .collect();
    let (signal, payload) = invoke(
        &router,
        case,
        caller,
        &json!({
            "probes": asked,
            "bucket": BUCKET,
            "key": TWO_COLOUR_KEY,
            "table": TABLE,
        }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );
    let report = payload["report"].clone();

    for probe in &probes {
        let case = probe.case;
        let reported = report[case].clone();
        assert_eq!(
            reported["bound"],
            json!(true),
            "case {case:?}: expected the bundled package to export the command class, got \
             {reported}"
        );
        let expected = if probe.served { "answered" } else { "refused" };
        assert_eq!(
            reported["outcome"].as_str(),
            Some(expected),
            "case {case:?}: expected the call to be {expected}, got {reported}"
        );
    }

    // The frame drawn in ONE colour: that colour, and the composition one colour adds up to.
    for case in [
        "one colour, staged as PNG",
        "the same frame staged in the other format the service reads",
    ] {
        let answer = report[case]["answer"].clone();
        assert_eq!(
            label_names(&answer),
            vec![RED_LABEL.to_string(), MONOCHROME_LABEL.to_string()],
            "case {case:?}: expected the colour drawn and the composition it adds up to, got \
             {answer}"
        );
        assert_eq!(
            answer["LabelModelVersion"].as_str(),
            Some(LABEL_MODEL_VERSION),
            "case {case:?}: expected the model that detected the labels, got {answer}"
        );
    }

    // Two colours, half the frame each: both, and no composition, because neither is the whole
    // frame — and what the handler read back is what this service answers a CLIENT for the same
    // image.
    let case_two = "two colours, neither of them the whole frame";
    let two = report[case_two]["answer"].clone();
    assert_eq!(
        label_names(&two),
        vec![RED_LABEL.to_string(), BLUE_LABEL.to_string()],
        "case {case_two:?}: expected both colours drawn and nothing else, got {two}"
    );
    let (status, directly) = rekognition(
        &router,
        "DetectLabels",
        &json!({ "Image": { "S3Object": { "Bucket": BUCKET, "Name": TWO_COLOUR_KEY } } }),
    )
    .await;
    assert_eq!(
        (status, label_names(&directly)),
        (200, label_names(&two)),
        "case {case_two:?}: expected the handler to have read what this service answers a \
         client for the same image, got {directly}"
    );

    // The same read narrowed: "the service returns the specified number of highest confidence
    // labels".
    let case_narrowed = "the same frame with the answer narrowed to one label";
    let narrowed = report[case_narrowed]["answer"].clone();
    assert_eq!(
        label_names(&narrowed),
        vec![RED_LABEL.to_string()],
        "case {case_narrowed:?}: expected the one highest-confidence label, got {narrowed}"
    );

    // The other analysis: the measured properties of the frame, and no label, because labels
    // were not asked for.
    let case_properties = "the other analysis of the same image, asked for by name";
    let properties = report[case_properties]["answer"].clone();
    assert!(
        properties["ImageProperties"]["Quality"]["Brightness"].is_number(),
        "case {case_properties:?}: expected the measured quality of the frame, got {properties}"
    );
    assert_eq!(
        properties["ImageProperties"]["DominantColors"][0]["SimplifiedColor"].as_str(),
        Some("red"),
        "case {case_properties:?}: expected the colour drawn as the frame's dominant one, got \
         {properties}"
    );
    assert_eq!(
        label_names(&properties),
        Vec::<String>::new(),
        "case {case_properties:?}: labels were not asked for, so none may be reported, got \
         {properties}"
    );
    assert!(
        properties["LabelModelVersion"].is_null(),
        "case {case_properties:?}: no label detection ran, so no model may be named, got \
         {properties}"
    );

    // Another read operation of the same service: a frame carrying no face is answered with an
    // empty list rather than a refusal.
    let case_faces = "another read operation of the same service";
    let faces = report[case_faces]["answer"].clone();
    assert_eq!(
        faces["FaceDetails"],
        json!([]),
        "case {case_faces:?}: expected the empty list a frame with no face measures to, got \
         {faces}"
    );

    // The write really minted the resource: the service's own list reports it.
    let case_write = "a write that mints a resource";
    let minted = report[case_write]["answer"].clone();
    assert_eq!(
        minted["StatusCode"],
        json!(200),
        "case {case_write:?}: expected the status the create reports, got {minted}"
    );
    let case_list = "the list that reports what the write minted";
    let listed = report[case_list]["answer"]["CollectionIds"].clone();
    assert_eq!(
        listed,
        json!([COLLECTION]),
        "case {case_list:?}: expected the collection the write minted, got {listed}"
    );

    // The refusal is the SERVICE's own: for each of the two classes an unreadable image falls
    // into, the modelled error class, its message and its status.
    let case_absent = "an image at an address the store does not serve";
    let refused = report[case_absent].clone();
    assert_eq!(
        (
            refused["name"].as_str(),
            refused["message"].as_str(),
            refused["status"].as_i64(),
            refused["unreadable_object"].as_bool(),
        ),
        (
            Some(UNREADABLE_OBJECT),
            Some(UNREADABLE_OBJECT_MESSAGE),
            Some(400),
            Some(true),
        ),
        "case {case_absent:?}: expected the service's own modelled refusal, got {refused}"
    );
    let case_malformed = "bytes that are no image at all";
    let malformed = report[case_malformed].clone();
    assert_eq!(
        (
            malformed["name"].as_str(),
            malformed["message"].as_str(),
            malformed["status"].as_i64(),
            malformed["unreadable_object"].as_bool(),
        ),
        (
            Some(INVALID_FORMAT),
            Some(INVALID_FORMAT_MESSAGE),
            Some(400),
            Some(false),
        ),
        "case {case_malformed:?}: expected the service's own modelled refusal for an unreadable \
         payload, got {malformed}"
    );

    // What the handler RECORDED, read back through DynamoDB's own API the way a client reads
    // it: the labels of the object the event named, under that object's own key.
    let held = dynamodb(
        &router,
        case,
        "GetItem",
        &json!({
            "TableName": TABLE,
            "Key": { "image_name": { "S": TWO_COLOUR_KEY } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        held["Item"]["labels"]["SS"],
        json!([RED_LABEL, BLUE_LABEL]),
        "case {case:?}: expected the measured labels recorded under the object's key, got {held}"
    );
    assert_eq!(
        held["Item"]["model_version"]["S"].as_str(),
        Some(LABEL_MODEL_VERSION),
        "case {case:?}: expected the model the service named to be recorded, got {held}"
    );
}

/// The OTHER runtime that bundles an SDK reaches the same service across the same seam: a
/// python handler's analysis of the staged frame and of bytes the event carried INLINE are
/// answered with the operation's own output members, an address the store does not serve is
/// refused with the service's own error code and message, and what the handler records is read
/// back where a client reads it.
#[tokio::test]
async fn a_python_handler_analyses_an_image_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a python handler calling Amazon Rekognition through the runtime's SDK";
    let router = emulator();
    let inline = staged(&router, case).await;

    let caller = "image-analysis-python-caller";
    create_function(
        &router,
        case,
        caller,
        "lambda_function.py",
        "lambda_function.lambda_handler",
        PYTHON_HANDLER,
        PYTHON_RUNTIME,
    )
    .await;

    let (signal, payload) = invoke(
        &router,
        case,
        caller,
        &json!({
            "bucket": BUCKET,
            "key": FLAT_PNG_KEY,
            "absent": ABSENT_KEY,
            "inline": inline,
            "table": TABLE,
        }),
    )
    .await;
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion with no function error, got \
         {signal:?} with {payload}"
    );
    assert_eq!(
        payload["recorded"],
        json!([RED_LABEL, MONOCHROME_LABEL]),
        "case {case:?}: expected the colour drawn and the composition it adds up to, got {payload}"
    );
    assert_eq!(
        payload["model_version"].as_str(),
        Some(LABEL_MODEL_VERSION),
        "case {case:?}: expected the model that detected the labels, got {payload}"
    );
    // The bytes the event carried inline are the two-colour frame, narrowed to one label.
    assert_eq!(
        payload["inline"],
        json!([RED_LABEL]),
        "case {case:?}: expected the one highest-confidence label of the inline frame, got \
         {payload}"
    );
    assert_eq!(
        (
            payload["refused"]["code"].as_str(),
            payload["refused"]["message"].as_str(),
            payload["refused"]["status"].as_i64(),
        ),
        (
            Some(UNREADABLE_OBJECT),
            Some(UNREADABLE_OBJECT_MESSAGE),
            Some(400),
        ),
        "case {case:?}: expected the service's own modelled refusal, got {payload}"
    );

    let held = dynamodb(
        &router,
        case,
        "GetItem",
        &json!({
            "TableName": TABLE,
            "Key": { "image_name": { "S": FLAT_PNG_KEY } },
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        held["Item"]["labels"]["SS"],
        json!([RED_LABEL, MONOCHROME_LABEL]),
        "case {case:?}: expected the measured labels recorded under the object's key, got {held}"
    );
}
