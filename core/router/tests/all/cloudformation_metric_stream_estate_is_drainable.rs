// SPDX-License-Identifier: CC0-1.0

//! A metric-stream estate a stack deployed carries the account's metric updates and can be
//! TORN DOWN: a running stream exports every metric update its filters admit — the ones the
//! service makes about its own API included — and once its streams are stopped the destination
//! bucket can be emptied and stays empty.
//!
//! Two rules, one data plane:
//!
//! 1. **A running metric stream exports every metric update recorded in the account's metric
//!    store that its filters select, whatever wrote it.** The three choices the operation
//!    documents are about NAMESPACES — "Stream metrics from all metric namespaces in the
//!    account", "… except for the namespaces that you list in `ExcludeFilters`", "… only the
//!    metric namespaces that you list in `IncludeFilters`" (`PutMetricStream`) — so a stream
//!    whose filters admit a namespace streams what is written there whoever wrote it, including
//!    the call count CloudWatch publishes about its own API, which no caller can publish and
//!    which is therefore the only metric update an estate nothing else touches ever sees.
//! 2. **A metric stream `StopMetricStreams` has taken out of service exports nothing further**
//!    — "The state of the metric stream. The possible values are `running` and `stopped`"
//!    (`GetMetricStreamOutput$State`) — so once every stream delivering into a destination is
//!    stopped, the objects already delivered are the LAST ones: a client that empties the
//!    bucket observes it empty and observes it STAY empty, and the stack that created it can
//!    then be deleted. That is what a teardown depends on: a bucket something keeps writing
//!    into can never be emptied, so the delete that follows never runs.
//!
//! Only a cross-service suite can ask this: the flow spans five services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — CloudFormation to deploy and destroy, IAM
//! for the role the export and the delivery are made under, CloudWatch for the streams, the
//! publishes and the stop, Firehose for the delivery stream, and Amazon S3 for the objects a
//! client lists and deletes.
//!
//! The rows differ along what the rules generalise over: how many streams deliver into the one
//! destination and which filter half each declares (none at all, `IncludeFilters`,
//! `ExcludeFilters` over a whole namespace and over a few of its metric names), whether the
//! destination writes under a key `Prefix`, whether its objects are compressed, and how many
//! datapoints in how many namespaces were published before the stop — so no row can pass by
//! delivering nothing at all, and the emptying is asserted over several delivered objects as
//! well as over one.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, outputs_of};
// The five-service router this flow spans, the SigV4 header its requests carry, the one request
// sender and the CloudFormation wire are the sibling suite's
// (`crate::cloudformation_metric_stream_exports_to_s3`, which drives the same pipeline): one
// definition of each, so the two suites cannot drift apart.
use crate::cloudformation_metric_stream_exports_to_s3::{
    cloudformation, emulator, send, CLOUDWATCH_SIGNING_NAME, CLOUDWATCH_TARGET_PREFIX,
};

/// How often a deployment is asked whether it has settled, and how long it goes on being
/// asked: a stack whose Firehose stream its owning service has not reported `ACTIVE` yet
/// completes from the instant that service does, which no fixed span names.
const SETTLE_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SETTLE_POLLS: usize = 600;

/// How long the condition each phase waits for is waited for before a row fails. Generous
/// against every span these services choose, because what a row waits for is the CONDITION,
/// re-read in a poll loop — a loaded or slower host must not fail a row the clock would have
/// served.
const CONDITION_DEADLINE: Duration = Duration::from_secs(90);

/// How often a condition is re-read while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long the destination is watched, after it has been emptied, to show that nothing more
/// arrives: a stopped stream exports nothing, so an object appearing in this window is the
/// defect this suite is about.
const SILENCE_WINDOW: Duration = Duration::from_secs(2);

/// The one-minute period an exported record covers (`MetricStat$Period`: "a period can be as
/// short as one minute (60 seconds) and must be a multiple of 60").
const PERIOD_SECS: f64 = 60.0;

/// The XML namespace an Amazon S3 request body carries.
const S3_XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The trust policy every row's role carries: Firehose assumes the role its destination names,
/// and a metric stream exports under the role it is configured with.
///
/// Written with no `&`, `=`, `+` or `%` so it travels in an `awsQuery` form body as written.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"#,
    r#"["firehose.amazonaws.com","streams.metrics.cloudwatch.amazonaws.com"]},"#,
    r#""Action":"sts:AssumeRole"}]}"#
);

/// The role's permissions: the record write each metric stream makes on the delivery stream
/// ("This IAM role must include the following permissions: firehose:PutRecord
/// firehose:PutRecordBatch" — `PutMetricStreamInput$RoleArn`) and the object write the delivery
/// itself makes on the destination bucket.
const ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["firehose:PutRecord","firehose:PutRecordBatch","s3:PutObject"],"Resource":"*"}]}"#
);

/// One metric stream a row's template declares: its logical id and the filter half it carries.
struct StreamDeclaration {
    logical_id: &'static str,
    name: &'static str,
    /// The filters member and its value, as the template declares it — `IncludeFilters` or
    /// `ExcludeFilters`, the two the model documents as mutually exclusive — or `None` for a
    /// stream that declares neither and so streams every namespace of the account.
    filters: Option<fn() -> (&'static str, Value)>,
}

/// One row of the table: an estate, what is published into it, and what the teardown then has
/// to be able to do.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    bucket: &'static str,
    role: &'static str,
    firehose: &'static str,
    /// The key prefix the destination writes under, when it declares one.
    prefix: Option<&'static str>,
    /// `CompressionFormat` the destination declares.
    compression: &'static str,
    /// The streams delivering into that one destination.
    streams: &'static [StreamDeclaration],
    /// What is published before the stop: `(namespace, metric name, values)`.
    published: &'static [(&'static str, &'static str, &'static [f64])],
}

/// The series CloudWatch publishes about its OWN API — the `AWS/Usage` call count, one
/// observation per served call — which no caller may publish into and which a stream whose
/// filters admit that namespace streams like any other metric of the account: "Stream metrics
/// from all metric namespaces in the account", or all but the ones `ExcludeFilters` names
/// (`PutMetricStream`).
const USAGE_NAMESPACE: &str = "AWS/Usage";
const USAGE_METRIC: &str = "CallCount";

/// The dimension of that series naming WHICH operation was called, so a record of it can be
/// tied to the call it counts.
const USAGE_RESOURCE_DIMENSION: &str = "Resource";

/// The namespace both filter halves of every row select, so every row delivers something.
const STREAMED_NAMESPACE: &str = "Custom/Estate";

/// A namespace a row's exclude filter drops, published beside the streamed one.
const DROPPED_NAMESPACE: &str = "Custom/Estate/Noise";

const ROWS: &[Row] = &[
    Row {
        case: "two streams, one include and one exclude, delivering uncompressed under a prefix",
        stack_name: "msdrain-pair",
        bucket: "msdrain-pair-metrics",
        role: "msdrain-pair-role",
        firehose: "msdrain-pair-firehose",
        prefix: Some("estate/metrics/"),
        compression: "UNCOMPRESSED",
        streams: &[
            StreamDeclaration {
                logical_id: "IncludeStream",
                name: "msdrain-pair-include",
                filters: Some(|| ("IncludeFilters", json!([{"Namespace": STREAMED_NAMESPACE}]))),
            },
            StreamDeclaration {
                logical_id: "ExcludeStream",
                name: "msdrain-pair-exclude",
                filters: Some(|| ("ExcludeFilters", json!([{"Namespace": DROPPED_NAMESPACE}]))),
            },
        ],
        published: &[
            (STREAMED_NAMESPACE, "Orders", &[3.0, 5.0, 11.0]),
            (DROPPED_NAMESPACE, "Chatter", &[7.0]),
        ],
    },
    Row {
        case: "one unfiltered stream delivering compressed objects with no prefix",
        stack_name: "msdrain-single",
        bucket: "msdrain-single-metrics",
        role: "msdrain-single-role",
        firehose: "msdrain-single-firehose",
        prefix: None,
        compression: "GZIP",
        streams: &[StreamDeclaration {
            logical_id: "EveryMetricStream",
            name: "msdrain-single-all",
            filters: None,
        }],
        published: &[(STREAMED_NAMESPACE, "Latency", &[42.0])],
    },
];

/// The metric names the recorded estate's exclude filter narrows its namespace to, in an order
/// no sort reproduces — the shape of filter a client writes to drop a few of one namespace's
/// metrics and stream everything else.
const EXCLUDED_METRIC_NAMES: [&str; 5] = [
    "NetworkPacketsOut",
    "CPUUtilization",
    "StatusCheckFailed",
    "DiskReadBytes",
    "NetworkIn",
];

/// The estates of the second table: one stream each, differing only in which filter half it
/// declares, so what each receives is decided by the filters alone.
///
/// Nothing is PUBLISHED into any of them: the only metric updates are the ones the service
/// makes about its own API, which is the point of the rows.
const SELF_PUBLISHED_ROWS: &[Row] = &[
    Row {
        case: "a stream declaring no filter at all",
        stack_name: "msself-all",
        bucket: "msself-all-metrics",
        role: "msself-all-role",
        firehose: "msself-all-firehose",
        prefix: Some("self/"),
        compression: "UNCOMPRESSED",
        streams: &[StreamDeclaration {
            logical_id: "EveryNamespaceStream",
            name: "msself-all-stream",
            filters: None,
        }],
        published: &[],
    },
    Row {
        case: "a stream excluding a few metric names of another namespace",
        stack_name: "msself-exclude",
        bucket: "msself-exclude-metrics",
        role: "msself-exclude-role",
        firehose: "msself-exclude-firehose",
        prefix: None,
        compression: "UNCOMPRESSED",
        streams: &[StreamDeclaration {
            logical_id: "ExcludeStream",
            name: "msself-exclude-stream",
            filters: Some(|| {
                (
                    "ExcludeFilters",
                    json!([{"Namespace": "AWS/EC2", "MetricNames": EXCLUDED_METRIC_NAMES}]),
                )
            }),
        }],
        published: &[],
    },
    Row {
        case: "a stream narrowed by IncludeFilters to one other namespace",
        stack_name: "msself-include",
        bucket: "msself-include-metrics",
        role: "msself-include-role",
        firehose: "msself-include-firehose",
        prefix: None,
        compression: "UNCOMPRESSED",
        streams: &[StreamDeclaration {
            logical_id: "IncludeStream",
            name: "msself-include-stream",
            filters: Some(|| ("IncludeFilters", json!([{"Namespace": STREAMED_NAMESPACE}]))),
        }],
        published: &[],
    },
];

/// The template one row deploys: the destination bucket, the role both the delivery and the
/// export are made under, the delivery stream, and the row's metric streams over it.
fn template(row: &Row) -> String {
    let mut destination = json!({
        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
        "CompressionFormat": row.compression,
        // The shortest interval the hint admits (`BufferingHints$IntervalInSeconds`, published
        // range 0 to 900), so a row asserting what the destination HOLDS does not wait out an
        // interval a consumer might configure instead.
        "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
    });
    if let Some(prefix) = row.prefix {
        destination["Prefix"] = json!(prefix);
    }
    let mut resources = json!({
        "DeliveryRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": row.role,
                "AssumeRolePolicyDocument": TRUST_POLICY,
                "Policies": [{"PolicyName": "export", "PolicyDocument": ROLE_POLICY}],
            },
        },
        "MetricsBucket": {
            "Type": "AWS::S3::Bucket",
            "Properties": {"BucketName": row.bucket},
        },
        "MetricsFirehose": {
            "Type": "AWS::KinesisFirehose::DeliveryStream",
            "Properties": {
                "DeliveryStreamName": row.firehose,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": destination,
            },
        },
    });
    let mut outputs = json!({
        "BucketName": {"Value": {"Ref": "MetricsBucket"}},
    });
    for stream in row.streams {
        let mut properties = json!({
            "Name": stream.name,
            "FirehoseArn": {"Fn::GetAtt": ["MetricsFirehose", "Arn"]},
            "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
            "OutputFormat": "json",
        });
        if let Some(filters) = stream.filters {
            let (member, value) = filters();
            properties[member] = value;
        }
        resources[stream.logical_id] = json!({
            "Type": "AWS::CloudWatch::MetricStream",
            "Properties": properties,
        });
        outputs[stream.logical_id] = json!({"Value": {"Ref": stream.logical_id}});
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a metric-stream estate over one Firehose destination",
        "Resources": resources,
        "Outputs": outputs,
    })
    .to_string()
}

/// Re-read `bucket` until the silence window closes, requiring it to be EMPTY on every pass.
///
/// A negative observation has no condition to wait for, so it is made REPEATEDLY rather than
/// once after a wait: each pass re-reads the destination, so an object that arrives at any point
/// in the window fails the caller, and the window itself is generous against the span a delivery
/// of this destination's own buffering needs.
async fn stays_empty(router: &MultiServiceRouter, case: &str, bucket: &str) {
    let until = Instant::now() + SILENCE_WINDOW;
    loop {
        let held = keys(router, case, bucket).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected {bucket} to stay empty once every stream was stopped, got \
             {held:?}"
        );
        if Instant::now() >= until {
            return;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One CloudWatch operation over its awsJson1_0 wire, as its STATUS and body — the seam this
/// suite needs and the sibling's own `cloudwatch` does not give, because several cases here
/// drive the service into a refusal and read what it said.
async fn cloudwatch(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
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
        serde_json::to_vec(request).expect("the request serializes"),
    )
    .await;
    let parsed = serde_json::from_slice(&answered).unwrap_or(Value::Null);
    (status, parsed)
}

/// The wire error code of a CloudWatch refusal: awsJson1_0 carries it as the body's `__type`.
fn cloudwatch_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Create the row's stack and answer the `DescribeStacks` body of its first settled
/// observation.
async fn create_and_settle(router: &MultiServiceRouter, row: &Row) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", row.stack_name),
            ("TemplateBody", &template(row)),
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

/// Every key `bucket` holds, read through `ListObjectsV2`, following the listing's own
/// continuation so a bucket holding more than one page is read whole.
async fn keys(router: &MultiServiceRouter, case: &str, bucket: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let path = match &token {
            Some(after) => format!(
                "/{bucket}?list-type=2&continuation-token={}",
                form_encode(after)
            ),
            None => format!("/{bucket}?list-type=2"),
        };
        let (status, body) = send(router, "s3", "GET", &path, &[], Vec::new()).await;
        let text = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListObjectsV2 on {bucket}, got {status} with \
             {text}"
        );
        found.extend(elements(&text, "Key"));
        let truncated = elements(&text, "IsTruncated")
            .first()
            .is_some_and(|flag| flag == "true");
        let next = elements(&text, "NextContinuationToken").first().cloned();
        match (truncated, next) {
            (true, Some(next)) => token = Some(next),
            _ => break,
        }
    }
    found.sort();
    found
}

/// Delete `held` from `bucket` with ONE `DeleteObjects` call — the multi-object delete a client
/// empties a bucket with — and answer the keys it reported deleted.
async fn delete_objects(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    held: &[String],
) -> Vec<String> {
    let objects: String = held
        .iter()
        .map(|key| format!("<Object><Key>{key}</Key></Object>"))
        .collect();
    let body = format!("<Delete xmlns=\"{S3_XMLNS}\">{objects}</Delete>");
    let (status, answered) = send(
        router,
        "s3",
        "POST",
        &format!("/{bucket}?delete"),
        &[("content-type", "application/xml")],
        body.into_bytes(),
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteObjects on {bucket}, got {status} with {text}"
    );
    elements(&text, "Key")
}

/// The records `bucket` holds, in key order: a delivered object is the records of one buffer
/// concatenated, and a `json` stream's record is one JSON document per line.
///
/// Only for a destination that writes its objects UNCOMPRESSED — a caller that asserts what a
/// record SAYS declares that spelling.
async fn records(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    held: &[String],
) -> Vec<Value> {
    let mut parsed = Vec::new();
    for key in held {
        let (status, body) =
            send(router, "s3", "GET", &format!("/{bucket}/{key}"), &[], Vec::new()).await;
        let text = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetObject for {key:?}, got {status} with {text}"
        );
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            parsed.push(serde_json::from_str(line).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: a json stream's record is one JSON document per line, got \
                     {line:?} ({error})"
                )
            }));
        }
    }
    parsed
}

/// Every stream `ListMetricStreams` reports, walked ONE entry at a time: the walk follows the
/// token each page hands back, and is bounded so a listing that never terminates fails the row
/// instead of hanging it.
///
/// "The token that marks the start of the next batch of returned results"
/// (`ListMetricStreamsOutput$NextToken`) is present only while a page is left, so a walk over a
/// finite estate ends.
async fn walk_metric_streams(router: &MultiServiceRouter, row: &Row) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut request = json!({"MaxResults": 1});
    // One page per stream, plus the page that ends the walk, plus margin: a walk that has not
    // ended by then is not ending.
    let pages = row.streams.len() + 8;
    for _ in 0..pages {
        let (status, listed) = cloudwatch(router, "ListMetricStreams", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from ListMetricStreams, got {status} with {listed}",
            row.case
        );
        for entry in listed.get("Entries").and_then(Value::as_array).into_iter().flatten() {
            let name = entry.get("Name").and_then(Value::as_str).unwrap_or_default();
            assert!(
                found.insert(name.to_string()),
                "case {:?}: expected each page to report a stream once, got {name:?} twice",
                row.case
            );
        }
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(token) => request = json!({"MaxResults": 1, "NextToken": token}),
            None => return found,
        }
    }
    panic!(
        "case {:?}: the listing kept handing back a token after {pages} pages, so a client \
         walking it never finishes; it reported {found:?}",
        row.case
    )
}

/// Whether `stream` streams the metrics of `namespace`, worked out from what the row DECLARES:/// the three choices the operation documents — every namespace of the account for a stream
/// declaring no filter, every one but those `ExcludeFilters` names, only those `IncludeFilters`
/// names (`PutMetricStream`).
fn streams_namespace(stream: &StreamDeclaration, namespace: &str) -> bool {
    let Some(filters) = stream.filters else {
        return true;
    };
    let (member, declared) = filters();
    let named = declared
        .as_array()
        .map(|filters| {
            filters
                .iter()
                .any(|filter| filter.get("Namespace").and_then(Value::as_str) == Some(namespace))
        })
        .unwrap_or_default();
    if member == "IncludeFilters" {
        named
    } else {
        !named
    }
}

/// A string member of one record.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Publish `published` — `(namespace, metric name, values)` entries — with every datum stamped
/// inside the SAME one-minute period, so the records they owe are delivered as soon as the
/// destination's buffer comes due.
async fn publish(
    router: &MultiServiceRouter,
    case: &str,
    published: &[(&str, &str, &[f64])],
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    let period_start = (now / PERIOD_SECS).floor() * PERIOD_SECS;
    for (namespace, metric_name, values) in published {
        let data: Vec<Value> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                json!({
                    "MetricName": metric_name,
                    "Value": value,
                    "Unit": "Count",
                    // Inside the period that opened at `period_start`, one second apart.
                    "Timestamp": period_start + index as f64,
                })
            })
            .collect();
        let (status, body) = cloudwatch(
            router,
            "PutMetricData",
            &json!({"Namespace": namespace, "MetricData": data}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from PutMetricData into {namespace:?}, got {status} \
             with {body}"
        );
    }
}

/// Wait until `bucket` holds at least one object, and answer what it then holds.
async fn delivered(router: &MultiServiceRouter, case: &str, bucket: &str) -> Vec<String> {
    let deadline = Instant::now() + CONDITION_DEADLINE;
    loop {
        let held = keys(router, case, bucket).await;
        if !held.is_empty() {
            return held;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the published datapoints to reach {bucket}, which stayed \
             empty"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Empty `bucket` the way a teardown does — list, delete what was listed, look again — and
/// answer every key that was ever listed, requiring the bucket to reach empty.
///
/// This is the loop a client's cleanup runs: a destination something keeps writing into never
/// reaches empty, so the loop is bounded by a deadline and the caller fails rather than hanging.
async fn drain(router: &MultiServiceRouter, case: &str, bucket: &str) -> BTreeSet<String> {
    let deadline = Instant::now() + CONDITION_DEADLINE;
    let mut emptied: BTreeSet<String> = BTreeSet::new();
    loop {
        let held = keys(router, case, bucket).await;
        if held.is_empty() {
            return emptied;
        }
        let deleted = delete_objects(router, case, bucket, &held).await;
        assert_eq!(
            deleted.len(),
            held.len(),
            "case {case:?}: expected every listed key to be reported deleted, listed {held:?} and \
             got {deleted:?}"
        );
        emptied.extend(held);
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {bucket} never reached empty — after deleting {} object(s) it still \
             lists objects, so a stopped estate is still delivering",
            emptied.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn a_stopped_metric_stream_estate_leaves_its_destination_emptiable() {
    for row in ROWS {
        let router = emulator();
        let described = create_and_settle(&router, row).await;
        assert_eq!(
            element(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {:?}: expected the estate's stack to complete, got {described}",
            row.case
        );
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("BucketName").map(String::as_str),
            Some(row.bucket),
            "case {:?}: expected the stack to report its destination bucket, got {described}",
            row.case
        );

        // Every declared stream is created running, which is what makes the publishes below
        // reach the destination at all.
        for stream in row.streams {
            let name = outputs
                .get(stream.logical_id)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: no {} output in {described}",
                        row.case, stream.logical_id
                    )
                })
                .clone();
            let (status, reported) =
                cloudwatch(&router, "GetMetricStream", &json!({"Name": &name})).await;
            assert_eq!(
                (status, reported.get("State").and_then(Value::as_str)),
                (200, Some("running")),
                "case {:?}: expected {name:?} to be running, got {status} with {reported}",
                row.case
            );
        }

        // The LISTING a client walks to find a stream by its filters: one entry per page, so
        // the walk follows the listing's own continuation and has to TERMINATE — a last page
        // that handed back a token would spin a client forever.
        let walked = walk_metric_streams(&router, row).await;
        let declared: BTreeSet<String> = row
            .streams
            .iter()
            .map(|stream| stream.name.to_string())
            .collect();
        assert_eq!(
            walked, declared,
            "case {:?}: expected the walk to report every stream of the estate once, got \
             {walked:?}",
            row.case
        );

        // What the estate delivered before it was stopped: at least one object, the records of
        // the publishes above.
        publish(&router, row.case, row.published).await;
        let before_stop = delivered(&router, row.case, row.bucket).await;

        // Stopping every stream of the estate is what quiesces delivery.
        let names: Vec<String> = row
            .streams
            .iter()
            .map(|stream| stream.name.to_string())
            .collect();
        let (status, stopped) =
            cloudwatch(&router, "StopMetricStreams", &json!({"Names": names})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from StopMetricStreams, got {status} with {stopped}",
            row.case
        );
        for stream in row.streams {
            let (status, reported) =
                cloudwatch(&router, "GetMetricStream", &json!({"Name": stream.name})).await;
            assert_eq!(
                (status, reported.get("State").and_then(Value::as_str)),
                (200, Some("stopped")),
                "case {:?}: expected {} to be stopped, got {status} with {reported}",
                row.case,
                stream.name
            );
        }

        // A publish after the stop is exported by nothing, so the drain below has only what
        // the estate delivered while it was running to remove.
        publish(&router, row.case, row.published).await;
        let emptied = drain(&router, row.case, row.bucket).await;
        assert!(
            emptied.iter().all(|key| !key.is_empty()),
            "case {:?}: every emptied key names an object, got {emptied:?}",
            row.case
        );
        assert!(
            before_stop.iter().all(|key| emptied.contains(key)),
            "case {:?}: expected the drain to remove what was delivered, delivered \
             {before_stop:?} and removed {emptied:?}",
            row.case
        );

        // And it STAYS empty: a stopped estate exports nothing, so nothing arrives for as long
        // as a delivery of this destination's own buffering needs, re-read throughout.
        stays_empty(&router, row.case, row.bucket).await;

        // An emptied destination is what lets the stack be deleted, and the streams go with it.
        let (status, deleted) =
            cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}",
            row.case
        );
        let deadline = Instant::now() + CONDITION_DEADLINE;
        loop {
            let (status, gone) = cloudwatch(
                &router,
                "GetMetricStream",
                &json!({"Name": row.streams[0].name}),
            )
            .await;
            if (status, cloudwatch_error_code(&gone).as_str()) == (404, "ResourceNotFoundException")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {:?}: expected {} to be gone with its stack, got {status} with {gone}",
                row.case,
                row.streams[0].name
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// How many metered calls each row of the second table makes: a handful, so the records the
/// destination holds cannot be one call's accident.
const METERED_CALLS: usize = 3;

#[tokio::test]
async fn a_running_stream_exports_the_metric_updates_the_service_makes_about_its_own_api() {
    for row in SELF_PUBLISHED_ROWS {
        let router = emulator();
        let described = create_and_settle(&router, row).await;
        assert_eq!(
            element(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {:?}: expected the stack to complete, got {described}",
            row.case
        );
        let stream = &row.streams[0];

        // CONTROL-PLANE calls only: nothing publishes a datapoint anywhere in this emulator,
        // so the only metric updates are the ones CloudWatch makes about its own API — one
        // call count per served call, in the namespace no caller may publish into.
        for _ in 0..METERED_CALLS {
            let (status, reported) =
                cloudwatch(&router, "GetMetricStream", &json!({"Name": stream.name})).await;
            assert_eq!(
                (status, reported.get("State").and_then(Value::as_str)),
                (200, Some("running")),
                "case {:?}: expected {} to be running, got {status} with {reported}",
                row.case,
                stream.name
            );
        }

        if !streams_namespace(stream, USAGE_NAMESPACE) {
            // The filters name another namespace, so these updates are none of this stream's:
            // the destination holds nothing, throughout a window as long as a delivery needs.
            stays_empty(&router, row.case, row.bucket).await;
            continue;
        }

        // The stream streams that namespace, so the destination holds those updates' records —
        // each naming the stream it came from, the series, and the operation it counts.
        let held = delivered(&router, row.case, row.bucket).await;
        let reported = records(&router, row.case, row.bucket, &held).await;
        assert!(
            !reported.is_empty(),
            "case {:?}: expected the delivered objects to carry records, got {held:?}",
            row.case
        );
        for record in &reported {
            assert_eq!(
                (
                    text(record, "namespace"),
                    text(record, "metric_name"),
                    text(record, "metric_stream_name"),
                ),
                (
                    USAGE_NAMESPACE.to_string(),
                    USAGE_METRIC.to_string(),
                    stream.name.to_string(),
                ),
                "case {:?}: every record here is the usage series of this stream, got {record}",
                row.case
            );
        }
        let resources: BTreeSet<String> = reported
            .iter()
            .filter_map(|record| {
                record
                    .get("dimensions")
                    .and_then(|dimensions| dimensions.get(USAGE_RESOURCE_DIMENSION))
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect();
        assert!(
            resources.contains("GetMetricStream"),
            "case {:?}: expected a record counting the reads above, got records for {resources:?}",
            row.case
        );

        // And the export STOPS with the stream: once it is stopped, a drained destination stays
        // drained however many further calls the service meters.
        let (status, stopped) =
            cloudwatch(&router, "StopMetricStreams", &json!({"Names": [stream.name]})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from StopMetricStreams, got {status} with {stopped}",
            row.case
        );
        drain(&router, row.case, row.bucket).await;
        for _ in 0..METERED_CALLS {
            cloudwatch(&router, "ListMetricStreams", &json!({})).await;
        }
        stays_empty(&router, row.case, row.bucket).await;
    }
}

/// The stack of the case below: it names NEITHER its destination bucket nor its streams, so
/// every handle a client has comes out of the stack's own `Outputs`.
fn generated_name_template() -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a metric-stream estate whose resources CloudFormation names",
        "Resources": {
            "DeliveryRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": TRUST_POLICY,
                    "Policies": [{"PolicyName": "export", "PolicyDocument": ROLE_POLICY}],
                },
            },
            "MetricsBucket": {"Type": "AWS::S3::Bucket"},
            "MetricsFirehose": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
                        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                        "CompressionFormat": "UNCOMPRESSED",
                        "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
                    },
                },
            },
            "ExcludeStream": {
                "Type": "AWS::CloudWatch::MetricStream",
                "Properties": {
                    "FirehoseArn": {"Fn::GetAtt": ["MetricsFirehose", "Arn"]},
                    "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                    "OutputFormat": "json",
                    "ExcludeFilters": [{"Namespace": DROPPED_NAMESPACE}],
                },
            },
        },
        "Outputs": {
            "BucketName": {"Value": {"Ref": "MetricsBucket"}},
            "StreamName": {"Value": {"Ref": "ExcludeStream"}},
        },
    })
    .to_string()
}

#[tokio::test]
async fn an_estate_whose_names_cloudformation_generated_is_read_and_drained_by_those_names() {
    // The handle a client is given for a resource it did not name is the stack's output, and
    // that handle has to be the name the OWNING service holds the resource under — otherwise
    // every later call a client makes with it (reading the stream, listing or emptying the
    // destination) addresses something that does not exist.
    let case = "a stack that names neither its bucket nor its stream";
    let stack_name = "msgen-stack";
    let router = emulator();
    let (status, created) = cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", &generated_name_template()),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {created}"
    );
    let mut described = String::new();
    for _ in 0..SETTLE_POLLS {
        let (status, body) =
            cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}"
        );
        described = body;
        let reported = element(&described, "StackStatus");
        if reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED") {
            break;
        }
        tokio::time::sleep(SETTLE_POLL_INTERVAL).await;
    }
    assert_eq!(
        element(&described, "StackStatus"),
        "CREATE_COMPLETE",
        "case {case:?}: expected the stack to complete, got {described}"
    );

    let outputs = outputs_of(&described);
    let bucket = outputs
        .get("BucketName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no BucketName output in {described}"));
    let stream = outputs
        .get("StreamName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no StreamName output in {described}"));
    assert!(
        !bucket.is_empty() && !stream.is_empty(),
        "case {case:?}: every generated name the stack reports is a handle, got {outputs:?}"
    );

    // The stream the stack reports is the stream CloudWatch holds, running.
    let (status, reported) =
        cloudwatch(&router, "GetMetricStream", &json!({"Name": &stream})).await;
    assert_eq!(
        (status, reported.get("State").and_then(Value::as_str)),
        (200, Some("running")),
        "case {case:?}: expected {stream:?} to be the running stream CloudWatch holds, got \
         {status} with {reported}"
    );

    // And the bucket the stack reports is the bucket Amazon S3 holds: the listing a client's
    // teardown starts from answers for it rather than refusing a name nothing was created under.
    // `keys` requires that 200.
    assert!(
        keys(&router, case, &bucket).await.is_empty(),
        "case {case:?}: a freshly created destination holds nothing yet"
    );

    // The whole lifecycle then runs against those generated names: publish, deliver, stop,
    // drain, and the destination stays empty.
    publish(
        &router,
        case,
        &[(STREAMED_NAMESPACE, "Orders", &[2.0, 4.0])],
    )
    .await;
    let before_stop = delivered(&router, case, &bucket).await;
    let (status, stopped) =
        cloudwatch(&router, "StopMetricStreams", &json!({"Names": [&stream]})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from StopMetricStreams, got {status} with {stopped}"
    );
    let emptied = drain(&router, case, &bucket).await;
    assert!(
        before_stop.iter().all(|key| emptied.contains(key)),
        "case {case:?}: expected the drain to remove what was delivered, delivered \
         {before_stop:?} and removed {emptied:?}"
    );
    stays_empty(&router, case, &bucket).await;
}
