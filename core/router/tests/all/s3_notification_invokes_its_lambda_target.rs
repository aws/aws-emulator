// SPDX-License-Identifier: CC0-1.0

//! An object write really RUNS the function a bucket's notification configuration names, end
//! to end over the assembled router — the configuration put through S3's own API, and the
//! configuration a CloudFormation stack declares.
//!
//! The rule under test: **a bucket whose notification configuration names a Lambda function
//! for an event class publishes each matching object event to that function as one
//! asynchronous invocation carrying the `Records` envelope, and only for an object whose key
//! every rule of the configuration's key-name `Filter` admits** — so the handler of a
//! configured function is handed the event, and a key the filter excludes reaches no function
//! at all.
//!
//! Four crates have to agree for that to hold (S3 evaluates the configuration and makes the
//! invocation, Lambda holds the grant and runs the handler, IAM answers for the execution role
//! the handler's own SDK call is signed as, and CloudFormation installs the declared form of
//! the same wiring), so the suite lives here: a crate-local suite can only ever see one side
//! of it. The S3 crate's `object_event_notifications` suite pins the invocation S3 MAKES; what
//! only this suite can reach is the handler really running on it, which it observes the way a
//! client does — the handler reads the object the event named and writes a marker object of
//! its own, read back through S3.
//!
//! The rows differ along what the rule generalises over: whether the configuration carries a
//! key-name filter at all, which rule kinds it carries (prefix, suffix, both), and whether the
//! written key satisfies every one of them. Each row asserts both halves, so a delivery that
//! never happens and one the filter excluded are told apart.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{form_encode, outputs_of};

/// The Region every request here is signed for. It is the Region the shared stack reader
/// ([`stack_status`]) signs for too, so the stack this suite creates is the stack it reports.
const REGION: &str = "us-east-1";

/// The XML namespace a client's S3 payload carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The `StackStatus` a deployed stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The service principal Amazon S3 publishes notifications under, and the action it performs
/// on a function destination.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The prefix the handler keys every marker it writes under.
const MARKER_PREFIX: &str = "markers/";

/// How long this suite waits for work the clock owes — a stack's resources coming up, and the
/// asynchronous delivery of an object event — and how often it looks while waiting.
const WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// How long a row that must see NOTHING keeps looking before concluding nothing came: well
/// past the span a delivered row takes to leave its marker.
const QUIET_WINDOW: Duration = Duration::from_secs(8);

/// The stack the emulator's own boot path gives the thread that assembles the router
/// (`core/router/src/main.rs`), kept at or above the size `boot_serve.rs` uses — the sibling
/// `s3_notification_pipeline_lifecycle` suite drives itself on the same size for the same
/// reason. Assembling every service and then dispatching a CHAIN of in-process calls through
/// it — CloudFormation into S3, S3's delivery into Lambda, the handler's own SDK call back
/// into S3 — nests more future than the default stack of a test thread holds.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Run `body` to completion on a thread with [`BOOT_STACK_SIZE`], on the current-thread
/// runtime this suite's deliveries are made on: a notification delivery is a task of its own,
/// so it runs on this same thread and needs the same depth.
///
/// One copy for both cases of this suite, so neither can drift onto a thread the chain it
/// drives does not fit on.
fn driven<Body>(name: &'static str, body: Body)
where
    Body: FnOnce() -> ::std::pin::Pin<Box<dyn ::std::future::Future<Output = ()>>>
        + Send
        + 'static,
{
    ::std::thread::Builder::new()
        .name(name.to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(move || {
            let runtime = ::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(body());
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread runs to completion");
}

/// The body every row writes, which the handler reads back off the object the event named.
const OBJECT_BODY: &str = "the object the event named";

/// The handler every row deploys: for each record it is handed it logs what it is processing
/// through the logging facility — the shape AWS documents for a Python handler — then reads the
/// object the event named and writes one marker object carrying what the event said about it,
/// through the AWS SDK the runtime supplies.
const MARKING_HANDLER: &str = r#"
import json
import logging
import os

import boto3

LOGGER = logging.getLogger()
LOGGER.setLevel(logging.INFO)


def handler(event, context):
    s3 = boto3.client("s3")
    written = []
    for record in event["Records"]:
        bucket = record["s3"]["bucket"]["name"]
        key = record["s3"]["object"]["key"]
        LOGGER.info("triggered for %s/%s", bucket, key)
        body = s3.get_object(Bucket=bucket, Key=key)["Body"].read().decode("utf-8")
        marker = {
            "eventSource": record["eventSource"],
            "eventName": record["eventName"],
            "bucket": bucket,
            "key": key,
            "body": body,
        }
        s3.put_object(
            Bucket=os.environ["MARKER_BUCKET"],
            Key="markers/" + key.replace("/", "_"),
            Body=json.dumps(marker).encode("utf-8"),
        )
        written.append(key)
    return {"written": written}
"#;

/// What that handler logs for one record, which a client reads out of the function's log group:
/// the level the record was emitted at, and the text the arguments were interpolated into.
const LOGGED_LEVEL: &str = "INFO";
fn logged_record(bucket: &str, key: &str) -> String {
    format!("triggered for {bucket}/{key}")
}

/// The emulator as it is ASSEMBLED, on one engine: the handler's SDK call is dispatched back
/// into this same router, so the bucket it writes is the bucket this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One response of S3's REST surface: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

/// Dispatch one request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// `PUT /<bucket>` — `CreateBucket`.
async fn create_bucket(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let answer = s3(router, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn put_object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str, body: &str) {
    let answer = s3(router, "PUT", &format!("/{bucket}/{key}"), body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// The keys `bucket` holds under [`MARKER_PREFIX`], read through `ListObjectsV2`.
async fn marker_keys(router: &MultiServiceRouter, case: &str, bucket: &str) -> Vec<String> {
    let answer = s3(
        router,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={MARKER_PREFIX}"),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: ListObjectsV2({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
    crate::query_wire::elements(&answer.body, "Key")
}

/// Wait until `bucket` holds a marker under [`MARKER_PREFIX`], and answer the keys it holds.
async fn awaited_markers(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    expected: bool,
) -> Vec<String> {
    let wait = if expected { WAIT } else { QUIET_WINDOW };
    let looks = wait.as_millis() / LOOK_INTERVAL.as_millis();
    let mut keys = Vec::new();
    for _ in 0..looks {
        keys = marker_keys(router, case, bucket).await;
        if expected && !keys.is_empty() {
            return keys;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    keys
}

/// The marker the handler left, parsed — and the assertions every delivered row owes it: it is
/// the record S3 published, and the handler really read the object that record named.
async fn assert_marker(
    router: &MultiServiceRouter,
    case: &str,
    markers: &str,
    keys: &[String],
    bucket: &str,
    key: &str,
) {
    assert_eq!(
        keys.len(),
        1,
        "case {case:?}: expected exactly one marker for the write of {key:?}, got {keys:?}"
    );
    let answer = s3(router, "GET", &format!("/{markers}/{}", keys[0]), "").await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetObject({markers:?}, {:?}) must succeed, got {} with {}",
        keys[0], answer.status, answer.body
    );
    let marker: Value = serde_json::from_str(&answer.body).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the marker must be the document the handler wrote ({error}), got {}",
            answer.body
        )
    });
    assert_eq!(
        marker["eventSource"].as_str(),
        Some("aws:s3"),
        "case {case:?}: expected the record's eventSource, got {marker}"
    );
    assert_eq!(
        marker["eventName"].as_str(),
        Some("ObjectCreated:Put"),
        "case {case:?}: expected the eventName of a put, got {marker}"
    );
    assert_eq!(
        marker["bucket"].as_str(),
        Some(bucket),
        "case {case:?}: expected the record to name the written bucket, got {marker}"
    );
    assert_eq!(
        marker["key"].as_str(),
        Some(key),
        "case {case:?}: expected the record to name the written key, got {marker}"
    );
    assert_eq!(
        marker["body"].as_str(),
        Some(OBJECT_BODY),
        "case {case:?}: the handler must have read the object the event named, got {marker}"
    );
}

/// One key-name `FilterRule`, as the request's XML carries it.
struct Rule {
    name: &'static str,
    value: &'static str,
}

/// `PUT /<bucket>?notification` with one Lambda configuration naming `function_arn` for
/// `s3:ObjectCreated:*`, filtered by `rules`.
///
/// The element is the one S3's own model binds the Lambda list to
/// (`smithy_query(query="shape",shape="NotificationConfiguration")`:
/// `LambdaFunctionConfigurations` is bound as `CloudFunctionConfiguration`, whose ARN member
/// is bound as `CloudFunction`).
async fn put_lambda_notification(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    function_arn: &str,
    rules: &[Rule],
) {
    let filter = if rules.is_empty() {
        String::new()
    } else {
        let written: String = rules
            .iter()
            .map(|rule| {
                format!(
                    "<FilterRule><Name>{}</Name><Value>{}</Value></FilterRule>",
                    rule.name, rule.value
                )
            })
            .collect();
        format!("<Filter><S3Key>{written}</S3Key></Filter>")
    };
    let body = format!(
        "<NotificationConfiguration xmlns=\"{S3_XMLNS}\"><CloudFunctionConfiguration>\
         <Id>marker</Id><CloudFunction>{function_arn}</CloudFunction>\
         <Event>s3:ObjectCreated:*</Event>{filter}\
         </CloudFunctionConfiguration></NotificationConfiguration>"
    );
    let answer = s3(router, "PUT", &format!("/{bucket}?notification"), &body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: PutBucketNotificationConfiguration({bucket:?}) must succeed, got {} \
         with {}",
        answer.status, answer.body
    );
}

/// The trust policy a Lambda execution role needs, and the grant the handler's own calls need.
fn role_resource() -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            },
            // The handler calls Amazon S3 with temporary credentials for this role, so the
            // role has to grant the read and the write it makes.
            "Policies": [{
                "PolicyName": "mark-the-event",
                "PolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": ["s3:GetObject", "s3:PutObject", "logs:*"],
                        "Resource": "*",
                    }],
                },
            }],
        },
    })
}

/// The function resource, whose inline code CloudFormation archives into its deployment
/// package.
fn function_resource(marker_bucket: &str, source: &str) -> Value {
    json!({
        "Type": "AWS::Lambda::Function",
        "Properties": {
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Role": { "Fn::GetAtt": ["HandlerRole", "Arn"] },
            "Timeout": 30,
            "Environment": { "Variables": { "MARKER_BUCKET": marker_bucket } },
            "Code": { "ZipFile": source },
        },
    })
}

/// A stack declaring the role, the function and the grant S3 needs — the wiring every row
/// shares — with `source_arn` scoping the grant when a row names one.
fn handler_stack(marker_bucket: &str, source_arn: Option<Value>, source: &str) -> Value {
    let mut permission = json!({
        "Type": "AWS::Lambda::Permission",
        "Properties": {
            "FunctionName": { "Fn::GetAtt": ["Handler", "Arn"] },
            "Action": INVOKE_ACTION,
            "Principal": S3_PRINCIPAL,
        },
    });
    if let Some(arn) = source_arn {
        permission["Properties"]["SourceArn"] = arn;
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "HandlerRole": role_resource(),
            "Handler": function_resource(marker_bucket, source),
            "InvokePermission": permission,
        },
        "Outputs": {
            "FunctionArn": { "Value": { "Fn::GetAtt": ["Handler", "Arn"] } },
            "FunctionName": { "Value": { "Ref": "Handler" } },
        },
    })
}

/// Create `stack` from `template`, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> std::collections::BTreeMap<String, String> {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = stack_status(router, stack).await;
    for _ in 0..looks {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        reported = stack_status(router, stack).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "case {case:?}: the stack must reach {CREATE_COMPLETE}, got {:?} with {}",
        reported.0, reported.1
    );
    outputs_of(&reported.1)
}

/// The `(FunctionArn, FunctionName)` outputs of the deployed handler stack.
async fn deployed_function(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &Value,
) -> (String, String) {
    let outputs = deployed(router, case, stack, &template.to_string()).await;
    let arn = outputs
        .get("FunctionArn")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the function's ARN"));
    let name = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the function's name"));
    (arn, name)
}

/// One row: the filter the configuration carries, the key written, and whether that key's
/// event must reach the function.
struct Row {
    case: &'static str,
    rules: &'static [Rule],
    key: &'static str,
    delivered: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "no filter at all admits every key",
        rules: &[],
        key: "anything.txt",
        delivered: true,
    },
    Row {
        case: "a prefix rule admits a key under it",
        rules: &[Rule {
            name: "prefix",
            value: "uploads/",
        }],
        key: "uploads/report.json",
        delivered: true,
    },
    Row {
        case: "a prefix rule excludes a key outside it",
        rules: &[Rule {
            name: "prefix",
            value: "uploads/",
        }],
        key: "archive/report.json",
        delivered: false,
    },
    Row {
        case: "every rule of a prefix-and-suffix filter has to hold",
        rules: &[
            Rule {
                name: "prefix",
                value: "uploads/",
            },
            Rule {
                name: "suffix",
                value: ".json",
            },
        ],
        key: "uploads/report.json",
        delivered: true,
    },
    Row {
        case: "a key the suffix rule of the filter excludes",
        rules: &[
            Rule {
                name: "prefix",
                value: "uploads/",
            },
            Rule {
                name: "suffix",
                value: ".json",
            },
        ],
        key: "uploads/report.csv",
        delivered: false,
    },
];

/// Every row: the write the configuration names reaches the function, whose handler leaves a
/// marker carrying what the event said — and a write the filter excludes leaves none.
///
/// Driven on a sized thread; see [`driven`].
#[test]
fn an_object_write_runs_the_function_the_bucket_notifies() {
    driven("s3-notification-invokes-its-lambda-target", || {
        Box::pin(an_object_write_runs_the_function_the_bucket_notifies_body())
    });
}

async fn an_object_write_runs_the_function_the_bucket_notifies_body() {
    let router = build_router();
    let markers = "notify-markers";
    create_bucket(&router, "the shared wiring", markers).await;
    let (function_arn, function_name) = deployed_function(
        &router,
        "the shared wiring",
        "notify-handler-stack",
        &handler_stack(markers, None, MARKING_HANDLER),
    )
    .await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let source = format!("notify-source-{index}");
        create_bucket(&router, case, &source).await;
        put_lambda_notification(&router, case, &source, &function_arn, row.rules).await;
        put_object(&router, case, &source, row.key, OBJECT_BODY).await;

        let keys = awaited_markers(&router, case, markers, row.delivered).await;
        if !row.delivered {
            assert!(
                keys.is_empty(),
                "case {case:?}: the filter excludes {:?}, so no function may run for it, got \
                 the marker(s) {keys:?}",
                row.key
            );
            continue;
        }
        assert_marker(&router, case, markers, &keys, &source, row.key).await;
        // The record the handler logged through the logging facility is in the function's own
        // log group, which is where a client reads that the write ran the function.
        assert_logged(&router, case, &function_name, &source, row.key).await;
        // The marker bucket is shared, so each row starts from an empty listing.
        for key in &keys {
            let removed = s3(&router, "DELETE", &format!("/{markers}/{key}"), "").await;
            assert_eq!(
                removed.status, 204,
                "case {case:?}: deleting the row's marker must succeed, got {} with {}",
                removed.status, removed.body
            );
        }
    }
}

/// The DEPLOYED form of the same wiring: a stack that declares the bucket, its
/// `NotificationConfiguration`, the function, the role and the grant reaches
/// `CREATE_COMPLETE`, and a write into the declared bucket runs the declared function.
///
/// Driven on a sized thread; see [`driven`].
#[test]
fn a_deployed_notification_configuration_runs_its_function() {
    driven("s3-deployed-notification-runs-its-function", || {
        Box::pin(a_deployed_notification_configuration_runs_its_function_body())
    });
}

async fn a_deployed_notification_configuration_runs_its_function_body() {
    let case = "a stack-declared notification configuration";
    let router = build_router();
    let markers = "deployed-markers";
    create_bucket(&router, case, markers).await;

    let stack = "deployed-notification-stack";
    let bucket = format!("{stack}-raw");
    let mut template = handler_stack(
        markers,
        Some(json!({ "Fn::Sub": format!("arn:aws:s3:::{bucket}") })),
        MARKING_HANDLER,
    );
    template["Resources"]["RawBucket"] = json!({
        "Type": "AWS::S3::Bucket",
        // The configuration S3 records is validated against the function's own policy, so the
        // grant has to be in place before the bucket is created with it.
        "DependsOn": "InvokePermission",
        "Properties": {
            "BucketName": bucket,
            "NotificationConfiguration": {
                "LambdaConfigurations": [{
                    "Event": "s3:ObjectCreated:*",
                    "Function": { "Fn::GetAtt": ["Handler", "Arn"] },
                    "Filter": {
                        "S3Key": { "Rules": [{ "Name": "prefix", "Value": "uploads/" }] },
                    },
                }],
            },
        },
    });
    let outputs = deployed(&router, case, stack, &template.to_string()).await;
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output the function's name"));

    let key = "uploads/deployed.json";
    put_object(&router, case, &bucket, key, OBJECT_BODY).await;
    let keys = awaited_markers(&router, case, markers, true).await;
    assert_marker(&router, case, markers, &keys, &bucket, key).await;
    assert_logged(&router, case, &function, &bucket, key).await;
}

/// One CloudWatch Logs call (`awsJson1_1`, `Logs_20140328.<Operation>`), as its status and
/// parsed body. A group the delivery has not created yet is a refusal this suite polls through,
/// so the status is answered rather than asserted here.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("logs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Logs_20140328.{operation}"))
        .header("authorization", authorization("logs"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Wait until the function's log group holds the record the handler logged for the write of
/// `key`, and assert the line it was rendered as — which is what a client polling the group
/// reads to see that the write ran the function.
async fn assert_logged(
    router: &MultiServiceRouter,
    case: &str,
    function: &str,
    bucket: &str,
    key: &str,
) {
    let group = format!("/aws/lambda/{function}");
    let wanted = logged_record(bucket, key);
    let opens = format!("[{LOGGED_LEVEL}]\t");
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut last = Value::Null;
    for _ in 0..looks {
        let (status, answered) = logs(
            router,
            "FilterLogEvents",
            &json!({ "logGroupName": group }),
        )
        .await;
        last = answered;
        if status == 200 {
            let messages: Vec<String> = last["events"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|event| event["message"].as_str().unwrap_or_default().to_string())
                .collect();
            if messages
                .iter()
                .any(|line| line.starts_with(&opens) && line.ends_with(&wanted))
            {
                return;
            }
            last = json!(messages);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    let rendered = format!(
        "case {case:?}: expected the group {group:?} to hold the record the handler logged \
         ({opens:?} … {wanted:?}), got {last}"
    );
    panic!("{rendered}");
}
