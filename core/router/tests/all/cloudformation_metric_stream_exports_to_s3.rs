// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed metric-stream export pipeline MOVES DATA: a datapoint published
//! after the stack completes lands as an object in the destination bucket.
//!
//! The rule under test: **a stack that reports `CREATE_COMPLETE` has really created the export
//! pipeline its template declares, so the `AWS::CloudWatch::MetricStream` it provisioned is a
//! stream in the `running` state — "When you use `PutMetricStream` to create a new metric
//! stream, the stream is created in the `running` state" — and a running stream exports every
//! metric update its filters select ("Metric streams can automatically stream CloudWatch
//! metrics to Amazon Web Services destinations, including Amazon S3", `PutMetricStream`)
//! through the `AWS::KinesisFirehose::DeliveryStream` its `FirehoseArn` names, whose own
//! destination then writes that buffer under its Amazon S3 prefix.** Nothing in the pipeline
//! is asked whether it is healthy: the records are asserted where a client sees them, as
//! objects of the bucket the stack created, read back through S3's own `ListObjectsV2` /
//! `GetObject`.
//!
//! Only a cross-service suite can ask this: the flow spans five services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — CloudFormation to deploy, IAM for the role
//! the export and the delivery are both made under, CloudWatch for the stream and the publish,
//! Firehose for the delivery stream, and Amazon S3 for the objects. A crate-local test only
//! ever sees one side of each call.
//!
//! The rows differ along what the rule generalises over: the template language (JSON, YAML with
//! the short-form intrinsic tags), whether the template NAMES the stream or leaves the name to
//! CloudFormation, which of the documented namespace filters the stream declares (none,
//! `IncludeFilters` narrowed to one metric name, `ExcludeFilters`), which spelling the delivery
//! stream declares its Amazon S3 destination with (`ExtendedS3DestinationConfiguration`, the
//! deprecated `S3DestinationConfiguration`), whether that destination declares a key `Prefix`,
//! and how many datapoints one period carries. Every row publishes something that must be
//! exported, so no row can pass by exporting nothing at all.
//!
//! Each row deploys into its OWN emulator: a stream declaring no filter streams every namespace
//! of its account and Region, which is exactly what one row asserts, so the rows cannot share a
//! store without one row's publishes being another row's records.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, outputs_of};

/// The Region every call below is signed for, and so the Region the stack's resources, their
/// ARNs and the exported records belong to.
pub(crate) const REGION: &str = "us-east-1";

/// CloudWatch's `X-Amz-Target` prefix, from its model's service shape
/// (`GraniteServiceVersion20100801`), and the signing name its credential scope carries
/// (`sigv4Name: monitoring`).
pub(crate) const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
pub(crate) const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// How often a deployment is asked whether it has settled, and how long it goes on being
/// asked: a stack whose Firehose stream its owning service has not reported `ACTIVE` yet
/// completes from the instant that service does, which no fixed span names.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SETTLE_POLLS: usize = 400;

/// How long a delivery is waited for before a row fails: far longer than the span a delivery
/// of a row's own buffering needs, so a loaded machine does not fail a row the clock would
/// have served.
///
/// A row's destination declares the shortest buffering interval the hint admits
/// ([`buffering_hints`]), which is the wait its delivery is then held for, so this budget is
/// about that wait and not about an interval a consumer might configure instead. Generous
/// against that wait: a host running the whole suite at once takes longer to serve it, and
/// what the row waits for is the CONDITION (the records arriving), re-read in a poll loop.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(90);

/// How often the destination bucket is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits, after the records it expects have arrived, before reading the
/// destination once more: what the stream's filters do NOT select is asserted absent after as
/// long a wait again as a delivery itself needs.
const SILENCE_WINDOW: Duration = Duration::from_secs(2);

/// The one-minute period an exported record covers (`MetricStat$Period`: "a period can be as
/// short as one minute (60 seconds) and must be a multiple of 60").
const PERIOD_SECS: f64 = 60.0;

/// The unit every datapoint below is published with, and so the one its record reports.
const UNIT: &str = "Count";

/// The trust policy every row's role carries: Firehose assumes the role its destination names,
/// and a metric stream exports under the role it is configured with.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"#,
    r#"["firehose.amazonaws.com","streams.metrics.cloudwatch.amazonaws.com"]},"#,
    r#""Action":"sts:AssumeRole"}]}"#
);

/// The role's permissions: the record write the metric stream makes on the delivery stream
/// ("This IAM role must include the following permissions: firehose:PutRecord
/// firehose:PutRecordBatch" — `PutMetricStreamInput$RoleArn`) and the object write the
/// delivery itself makes on the bucket.
const ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["firehose:PutRecord","firehose:PutRecordBatch","s3:PutObject"],"Resource":"*"}]}"#
);

/// One row: the stack it deploys, what it publishes once that stack completes, and which of
/// those publishes the destination bucket must then hold a record for.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The bucket the template declares, which is also where the records are read from.
    bucket: &'static str,
    /// The key `Prefix` the destination declares, or `None` where it declares none.
    prefix: Option<&'static str>,
    /// The template body, built from the row's own names.
    template: fn() -> String,
    /// What `PutMetricData` publishes: one entry per metric, with the values it carries in
    /// ONE period.
    published: &'static [(&'static str, &'static str, &'static [f64])],
    /// The `(namespace, metric name)` pairs a record must arrive for; every other published
    /// metric is one no record may arrive for.
    exported: &'static [(&'static str, &'static str)],
    /// Whether this row's filters leave the account's OWN usage namespace in, so the
    /// destination also holds records of the call count CloudWatch publishes about its own API
    /// ([`USAGE_METRIC`]): a stream "from all metric namespaces in the account" streams that
    /// namespace like any other, and a stream narrowed to another namespace by `IncludeFilters`
    /// does not. How MANY such records arrive is not a row's to state — it is one per metered
    /// call of the period — so only their presence or absence is asserted.
    streams_usage: bool,
}

/// The series CloudWatch publishes about its own API: the `AWS/Usage` call count
/// (`crate::…::state::usage`, the service's own metering), which a stream selecting that
/// namespace streams.
const USAGE_METRIC: (&str, &str) = ("AWS/Usage", "CallCount");

/// The JSON template the first two rows share: the delivery role with the permissions the
/// export and the delivery need, the destination bucket, the Firehose stream writing into it,
/// and the metric stream pointed at that Firehose stream.
///
/// `destination` is the member the delivery stream declares its Amazon S3 destination under
/// and the configuration itself; `stream_properties` is the metric stream's own declaration.
fn json_template(
    role_name: &str,
    bucket: &str,
    firehose_name: &str,
    destination: (&str, Value),
    stream_properties: Value,
) -> String {
    let (destination_member, destination_configuration) = destination;
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a CloudWatch metric stream exporting through Firehose to S3",
        "Resources": {
            "DeliveryRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": role_name,
                    "AssumeRolePolicyDocument": TRUST_POLICY,
                    "Policies": [{
                        "PolicyName": "export",
                        "PolicyDocument": ROLE_POLICY,
                    }],
                },
            },
            "MetricsBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": bucket},
            },
            "MetricsStream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamName": firehose_name,
                    "DeliveryStreamType": "DirectPut",
                    destination_member: destination_configuration,
                },
            },
            "MetricStream": {
                "Type": "AWS::CloudWatch::MetricStream",
                "Properties": stream_properties,
            },
        },
        "Outputs": {
            "StreamName": {"Value": {"Ref": "MetricStream"}},
            "BucketName": {"Value": {"Ref": "MetricsBucket"}},
        },
    })
    .to_string()
}

/// The buffering every destination below declares, with the SHORTEST interval the hint admits
/// (`BufferingHints$IntervalInSeconds`, whose published range is 0 to 900): "buffer incoming
/// data for the specified period of time, in seconds, before delivering it to the destination"
/// is the wait a delivery is held for, so a row that asserts what the destination holds
/// declares the shortest one rather than waiting out a consumer's own interval.
fn buffering_hints() -> Value {
    json!({"SizeInMBs": 1, "IntervalInSeconds": 0})
}

const ROWS: &[Row] = &[
    Row {
        case: "a json stream declaring no filter, delivering under a declared prefix",
        stack_name: "ms-export-all",
        bucket: "ms-export-all-metrics",
        prefix: Some("exports/"),
        template: || {
            json_template(
                "ms-export-all-role",
                "ms-export-all-metrics",
                "ms-export-all-firehose",
                (
                    "ExtendedS3DestinationConfiguration",
                    json!({
                        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
                        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                        "Prefix": "exports/",
                        "BufferingHints": buffering_hints(),
                    }),
                ),
                json!({
                    "Name": "ms-export-all-stream",
                    "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
                    "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                    "OutputFormat": "json",
                }),
            )
        },
        published: &[
            ("Custom/Orders", "Placed", &[7.0, 11.0, 3.0]),
            ("Custom/Carts", "Abandoned", &[2.0]),
        ],
        exported: &[("Custom/Orders", "Placed"), ("Custom/Carts", "Abandoned")],
        // No filter at all: every namespace of the account is streamed, the reserved one its
        // own usage metrics live in included.
        streams_usage: true,
    },
    Row {
        case: "an UNNAMED json stream including one namespace narrowed to one metric name, \
               delivering through the deprecated S3 destination spelling with no prefix",
        stack_name: "ms-export-included",
        bucket: "ms-export-included-metrics",
        prefix: None,
        template: || {
            json_template(
                "ms-export-included-role",
                "ms-export-included-metrics",
                "ms-export-included-firehose",
                (
                    "S3DestinationConfiguration",
                    json!({
                        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
                        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                        "BufferingHints": buffering_hints(),
                    }),
                ),
                json!({
                    "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
                    "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                    "OutputFormat": "json",
                    "IncludeFilters": [
                        {"Namespace": "Custom/Billing", "MetricNames": ["Invoices"]},
                    ],
                }),
            )
        },
        published: &[
            ("Custom/Billing", "Invoices", &[5.0]),
            ("Custom/Billing", "Drafts", &[9.0]),
            ("Custom/Audit", "Events", &[1.0]),
        ],
        exported: &[("Custom/Billing", "Invoices")],
        // `IncludeFilters` names one namespace, so nothing else is streamed.
        streams_usage: false,
    },
    Row {
        case: "a yaml stream with short-form intrinsics excluding one namespace",
        stack_name: "ms-export-excluded",
        bucket: "ms-export-excluded-metrics",
        prefix: Some("firehose/metrics/"),
        template: || {
            format!(
                "\
AWSTemplateFormatVersion: '2010-09-09'
Description: a metric stream export pipeline declared in YAML
Resources:
  DeliveryRole:
    Type: AWS::IAM::Role
    Properties:
      RoleName: ms-export-excluded-role
      AssumeRolePolicyDocument: '{TRUST_POLICY}'
      Policies:
        - PolicyName: export
          PolicyDocument: '{ROLE_POLICY}'
  MetricsBucket:
    Type: AWS::S3::Bucket
    Properties:
      BucketName: ms-export-excluded-metrics
  MetricsStream:
    Type: AWS::KinesisFirehose::DeliveryStream
    Properties:
      DeliveryStreamName: ms-export-excluded-firehose
      DeliveryStreamType: DirectPut
      ExtendedS3DestinationConfiguration:
        BucketARN: !GetAtt MetricsBucket.Arn
        RoleARN: !GetAtt DeliveryRole.Arn
        Prefix: firehose/metrics/
        CompressionFormat: UNCOMPRESSED
        BufferingHints:
          SizeInMBs: 1
          IntervalInSeconds: 0
  MetricStream:
    Type: AWS::CloudWatch::MetricStream
    Properties:
      Name: ms-export-excluded-stream
      FirehoseArn: !GetAtt MetricsStream.Arn
      RoleArn: !GetAtt DeliveryRole.Arn
      OutputFormat: json
      ExcludeFilters:
        - Namespace: Custom/Noise
Outputs:
  StreamName:
    Value: !Ref MetricStream
  BucketName:
    Value: !Ref MetricsBucket
"
            )
        },
        published: &[
            ("Custom/Fleet", "Trips", &[4.0, 6.0]),
            ("Custom/Noise", "Chatter", &[8.0]),
        ],
        exported: &[("Custom/Fleet", "Trips")],
        // `ExcludeFilters` names another namespace, so the usage namespace is still streamed.
        streams_usage: true,
    },
];

/// The emulator one row drives: the five services the flow spans, on ONE engine.
///
/// `pub(crate)` with the three helpers below it: the metric-stream estate suite
/// (`crate::cloudformation_metric_stream_estate_is_drainable`) drives the same five services
/// over the same wire, and one definition of the router and of the request builders is what
/// keeps the two suites from drifting apart.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not any of these services'.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the routing
/// key the router claims a request by. Neither the key id nor the signature is a credential:
/// this router runs with request authentication off, so nothing here is verified.
pub(crate) fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One request to the service `signing_name` serves; answers its status and body bytes.
pub(crate) async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.body().to_vec())
}

/// One CloudFormation action over its awsQuery wire. The form components are encoded by the
/// test crate's one awsQuery encoder, so a template body full of `{`, `"` and `&` survives it.
pub(crate) async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = send(
        router,
        "cloudformation",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await;
    (status, String::from_utf8_lossy(&answered).to_string())
}

/// One CloudWatch operation over its awsJson1_0 wire, requiring a 200.
async fn cloudwatch(router: &MultiServiceRouter, case: &str, operation: &str, request: &Value) {
    let (status, answered) = send(
        router,
        CLOUDWATCH_SIGNING_NAME,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            (
                "x-amz-target",
                &format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
            ),
        ],
        request.to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: expected HTTP 200 from {operation}, got {status} with {}",
        String::from_utf8_lossy(&answered)
    );
}

/// Create the row's stack and answer the `DescribeStacks` body of the first SETTLED
/// observation of it.
async fn create_and_settle(router: &MultiServiceRouter, row: &Row) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", row.stack_name),
            ("TemplateBody", &(row.template)()),
            // Every row's template declares the IAM role the export and the delivery are made
            // under, by name, so the request acknowledges the template's IAM resources.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from CreateStack, got {status} with body {body}",
        row.case
    );
    let mut described = describe_stack(router, row).await;
    for _ in 0..SETTLE_POLLS {
        let status = element(&described, "StackStatus");
        if status.ends_with("_COMPLETE") || status.ends_with("_FAILED") {
            return described;
        }
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
        described = describe_stack(router, row).await;
    }
    described
}

/// The one stack the row deployed, as the response body.
async fn describe_stack(router: &MultiServiceRouter, row: &Row) -> String {
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", row.stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}",
        row.case
    );
    body
}

/// Every key the row's bucket holds, read through `ListObjectsV2`.
///
/// The listing is read in one page and each key is used in a request path as written: a row
/// delivers a handful of objects, and a delivered object's key is the destination's prefix plus
/// the automatic time segments plus the object's own name, which carry nothing a path has to
/// escape. A row that ever delivered more than one page would fail the assertions below rather
/// than pass on a short read.
async fn keys(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{}?list-type=2", row.bucket),
        &[],
        Vec::new(),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from ListObjectsV2 on {}, got {status} with {text}",
        row.case, row.bucket
    );
    let mut found = elements(&text, "Key");
    found.sort();
    found
}

/// The records the row's bucket holds, in key order: a delivered object is the records of one
/// buffer concatenated, and each record is one JSON document on its own line.
async fn delivered(router: &MultiServiceRouter, row: &Row) -> (Vec<String>, Vec<Value>) {
    let held = keys(router, row).await;
    let mut records = Vec::new();
    for key in &held {
        let (status, body) = send(
            router,
            "s3",
            "GET",
            &format!("/{}/{key}", row.bucket),
            &[],
            Vec::new(),
        )
        .await;
        let text = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetObject for {key:?}, got {status} with {text}",
            row.case
        );
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: Value = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!(
                    "case {:?}: every line of {key:?} must be a JSON record, got {line:?}: \
                     {error}",
                    row.case
                )
            });
            records.push(record);
        }
    }
    (held, records)
}

/// What the row's destination holds at the END of the silence window, re-read throughout it.
///
/// The window is what gives a record the row's filters withhold time to arrive if the service
/// wrongly exported one; it is spent RE-READING the destination rather than sleeping once, so the
/// read the assertions judge is the latest one and nothing is asserted about how fast the host
/// ran.
async fn held_through_the_silence(
    router: &MultiServiceRouter,
    row: &Row,
) -> (Vec<String>, Vec<Value>) {
    let until = Instant::now() + SILENCE_WINDOW;
    let mut answer = delivered(router, row).await;
    loop {
        if Instant::now() >= until {
            return answer;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        answer = delivered(router, row).await;
    }
}

/// The row's delivered records once every metric in `want` has one — or a failure naming the
/// row once [`DELIVERY_DEADLINE`] has passed.
///
/// The condition is the metrics rather than a COUNT: a destination also holds the records of
/// the account's own usage series where the row's filters leave that namespace in
/// ([`Row::streams_usage`]), so a count would be satisfied by records of a metric the row is
/// not waiting for.
async fn delivered_records(
    router: &MultiServiceRouter,
    row: &Row,
    want: &BTreeSet<(String, String)>,
) -> (Vec<String>, Vec<Value>) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let (held, records) = delivered(router, row).await;
        let seen: BTreeSet<(String, String)> = records.iter().map(metric_of).collect();
        if want.iter().all(|metric| seen.contains(metric)) {
            return (held, records);
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected an exported record for each of {want:?} in {}, got {records:?} \
             under keys {held:?}",
            row.case,
            row.bucket
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A string member of a record.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `(namespace, metric_name)` a record reports.
fn metric_of(record: &Value) -> (String, String) {
    (text(record, "namespace"), text(record, "metric_name"))
}

/// One statistic of a record's `value` object.
fn statistic(record: &Value, name: &str) -> Option<f64> {
    record.get("value")?.get(name)?.as_f64()
}

/// The instant the period one period before the current one opens at, as the epoch seconds
/// the awsJson protocols carry a timestamp as.
///
/// A whole period back, and every datum of a row stamped inside it, so no publish of a row
/// falls in a different period from another because the clock crossed a boundary mid-case.
fn period_start() -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    (now / PERIOD_SECS).floor() * PERIOD_SECS - PERIOD_SECS
}

/// The four statistics "a metric stream always sends … `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT`"
/// (`PutMetricStream`), computed here from what the row published.
fn expected_statistics(values: &[f64]) -> [(&'static str, f64); 4] {
    let sum: f64 = values.iter().sum();
    let max = values.iter().copied().fold(f64::MIN, f64::max);
    let min = values.iter().copied().fold(f64::MAX, f64::min);
    [
        ("max", max),
        ("min", min),
        ("sum", sum),
        ("count", values.len() as f64),
    ]
}

/// Whether `key` carries the automatic time prefix a delivered object is written under: "The
/// \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon S3 files"
/// (`S3DestinationConfiguration$Prefix`) — four digits, then two, two and two, each its own
/// segment, before the object's own name.
fn carries_the_time_prefix(key: &str) -> bool {
    let segments: Vec<&str> = key.split('/').collect();
    let widths = [4usize, 2, 2, 2];
    segments.len() > widths.len()
        && segments.iter().zip(widths).all(|(segment, width)| {
            segment.len() == width && segment.chars().all(|digit| digit.is_ascii_digit())
        })
}

#[tokio::test]
async fn a_deployed_metric_stream_exports_its_publishes_through_firehose_to_s3() {
    let account = engine::emulator_account_id();
    for row in ROWS {
        let case = row.case;
        let router = emulator();

        // The pipeline, deployed exactly as a client deploys it.
        let described = create_and_settle(&router, row).await;
        let stack_status = element(&described, "StackStatus");
        assert_eq!(
            stack_status, "CREATE_COMPLETE",
            "case {case:?}: the pipeline's stack must complete, got {stack_status} with body \
             {described}"
        );
        let outputs = outputs_of(&described);
        let stream_name = outputs.get("StreamName").cloned().unwrap_or_default();
        assert!(
            !stream_name.is_empty(),
            "case {case:?}: the stack must report the deployed stream's name, got {outputs:?}"
        );
        assert_eq!(
            outputs.get("BucketName").map(String::as_str),
            Some(row.bucket),
            "case {case:?}: the stack must report the destination bucket it created, got \
             {outputs:?}"
        );

        // One publish per metric, every datum stamped in the same period, so each selected
        // metric owes exactly one record.
        // The period is read ONCE and both the publish and the expectation are built from it:
        // a row spends the delivery wait between the two, so a boundary the clock crosses in
        // that span must not move what the record is expected to be stamped with.
        let period = period_start();
        let stamp = period + PERIOD_SECS / 2.0;
        for (namespace, metric_name, values) in row.published {
            let data: Vec<Value> = values
                .iter()
                .map(|value| {
                    json!({
                        "MetricName": metric_name,
                        "Value": value,
                        "Unit": UNIT,
                        "Timestamp": stamp,
                    })
                })
                .collect();
            cloudwatch(
                &router,
                case,
                "PutMetricData",
                &json!({"Namespace": namespace, "MetricData": data}),
            )
            .await;
        }

        // What the destination holds once the records the row expects have arrived, RE-READ for
        // as long as a delivery needs so a record the filters withhold cannot pass by being
        // late: the last read of that window is what the assertions below judge.
        let published_metrics: BTreeSet<(String, String)> = row
            .exported
            .iter()
            .map(|(namespace, metric_name)| ((*namespace).to_string(), (*metric_name).to_string()))
            .collect();
        delivered_records(&router, row, &published_metrics).await;
        let (held, records) = held_through_the_silence(&router, row).await;

        let seen: BTreeSet<(String, String)> = records.iter().map(metric_of).collect();
        // Beside the row's own publishes, the destination holds the account's usage series
        // wherever the row's filters leave that namespace in — a stream streams the metrics of
        // every namespace its filters admit, whoever published them.
        let mut expected = published_metrics.clone();
        if row.streams_usage {
            expected.insert((USAGE_METRIC.0.to_string(), USAGE_METRIC.1.to_string()));
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected a record for exactly {expected:?}, got {seen:?} in \
             {records:?}"
        );
        // One record per selected metric of one period, counted over the metrics the row
        // published: the usage series is counted per metered call rather than per period, so
        // its records are left out of this count.
        let published_records = records
            .iter()
            .filter(|record| published_metrics.contains(&metric_of(record)))
            .count();
        assert_eq!(
            published_records,
            row.exported.len(),
            "case {case:?}: one selected metric of one period owes one record, got {records:?}"
        );

        // Every metric the row's filters select, read from the records themselves: a pair the
        // destination holds no record for is a failure here rather than a row that quietly
        // asserts nothing about it.
        for (namespace, metric_name) in row.exported {
            let values = row
                .published
                .iter()
                .find(|(published_namespace, published_name, _)| {
                    published_namespace == namespace && published_name == metric_name
                })
                .map(|(_, _, values)| *values)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {namespace}/{metric_name} must be one the row published")
                });
            let reported = ((*namespace).to_string(), (*metric_name).to_string());
            let record = records
                .iter()
                .find(|record| metric_of(record) == reported)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the destination must hold a record for \
                         {namespace}/{metric_name}, got {records:?}"
                    )
                });
            assert_eq!(
                text(record, "metric_stream_name"),
                stream_name,
                "case {case:?}: the record of {namespace}/{metric_name} must name the deployed \
                 stream {stream_name:?}, got {record:?}"
            );
            assert_eq!(
                text(record, "region"),
                REGION,
                "case {case:?}: the record of {namespace}/{metric_name} must name the Region it \
                 was published in, got {record:?}"
            );
            assert_eq!(
                text(record, "account_id"),
                account,
                "case {case:?}: the record of {namespace}/{metric_name} must name this account, \
                 got {record:?}"
            );
            assert_eq!(
                text(record, "unit"),
                UNIT,
                "case {case:?}: the record of {namespace}/{metric_name} must report the unit the \
                 datapoints were published with, got {record:?}"
            );
            let stamped = record.get("timestamp").and_then(Value::as_i64);
            assert_eq!(
                stamped,
                Some((period * 1_000.0) as i64),
                "case {case:?}: the record of {namespace}/{metric_name} must be stamped with the \
                 instant its period opens at, got {record:?}"
            );
            for (statistic_name, expected_value) in expected_statistics(values) {
                assert_eq!(
                    statistic(record, statistic_name),
                    Some(expected_value),
                    "case {case:?}: the record of {namespace}/{metric_name} must report \
                     {statistic_name}={expected_value}, got {record:?}"
                );
            }
        }

        // Every object is written where the destination says, under the prefix it declared and
        // the automatic time prefix below it.
        for key in &held {
            let prefix = row.prefix.unwrap_or_default();
            assert!(
                key.starts_with(prefix),
                "case {case:?}: a delivered object must be written under {prefix:?}, got {key:?}"
            );
            let below = &key[prefix.len()..];
            assert!(
                carries_the_time_prefix(below),
                "case {case:?}: a delivered object must carry the automatic YYYY/MM/DD/HH \
                 prefix, got {key:?}"
            );
        }
    }
}
