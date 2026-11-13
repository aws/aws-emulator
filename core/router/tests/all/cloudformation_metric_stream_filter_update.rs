// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation-deployed metric-stream pipeline is usable the instant the stack reports
//! itself complete, and it exports what its CURRENT declaration selects — before and after a
//! stack update rewrites the filter.
//!
//! The rules under test, both stated about the pipeline a client deploys and then uses:
//!
//! 1. **CloudFormation reports a resource — and so the stack operation holding it — created only
//!    once the service that owns it reports the resource in the state it is usable in.** For an
//!    `AWS::KinesisFirehose::DeliveryStream` that state is `ACTIVE`: `CreateDeliveryStream` "is
//!    an asynchronous operation that immediately returns. The initial status of the Firehose
//!    stream is CREATING. After the Firehose stream is created, its status is ACTIVE and it now
//!    accepts data … Attempts to send data to a delivery stream that is not in the ACTIVE state
//!    cause an exception." So the FIRST observation of the completed stack is followed here by a
//!    single `DescribeDeliveryStream` — no polling, because a client that was told the
//!    deployment finished does not poll.
//! 2. **A running metric stream exports exactly the metric updates the filters of its current
//!    configuration select, and a stack update that rewrites those filters changes what the
//!    deployed pipeline delivers from then on.** The three choices are the operation's own:
//!    "Stream metrics from all metric namespaces in the account", "… except for the namespaces
//!    that you list in `ExcludeFilters`", "… only the metric namespaces that you list in
//!    `IncludeFilters`", narrowed per namespace by "If you omit this parameter, all metrics in
//!    the namespace are included or excluded" (`MetricStreamFilter$MetricNames`); and
//!    `PutMetricStream` is "Creates or updates a metric stream … If you use it to update an
//!    existing stream, the state of the stream is not changed", which is the call a changed
//!    declaration reaches CloudWatch as.
//!
//! Only a cross-service suite can ask either one: the flow spans CloudFormation (to deploy and
//! to update), IAM (the role the export and the delivery are made under), CloudWatch (the stream
//! and the publishes), Firehose (the delivery stream) and Amazon S3 (where the records land) over
//! ONE engine, exactly as `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what rule 2 generalises over — HOW the update rewrites the filter: a
//! namespace added to an `IncludeFilters`, a per-namespace metric-name narrowing widened to the
//! whole namespace, two included namespaces narrowed to one, and an `ExcludeFilters` replaced by
//! an `IncludeFilters` naming the very namespace it had excluded. Every row publishes into a
//! namespace the filter does NOT select in each phase, so no row can pass by exporting
//! everything, and every row publishes into one it DOES select, so none can pass by exporting
//! nothing.
//!
//! Each phase publishes into its own one-minute period, so a record is told from its sibling of
//! the other phase by the instant it is stamped with rather than by its arrival.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this test crate writes ONCE
// (`tests/all/query_wire.rs`), and the SigV4 header builder it writes once beside them, which
// signs for the same Region this suite's hosts name.
use crate::query_wire::{element, elements, form_encode, outputs_of};
use crate::rds_docdb_neptune_signing_name_routing::authorization;

/// The Region every call below is signed for, and so the Region the stack's resources, their
/// ARNs and the exported records belong to.
const REGION: &str = "us-east-1";

/// CloudWatch's `X-Amz-Target` prefix, from its model's service shape
/// (`GraniteServiceVersion20100801`), and the signing name its credential scope carries
/// (`sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// Amazon Data Firehose's `X-Amz-Target` prefix (service shape `Firehose_20150804`) and its
/// signing name (`sigv4Name: firehose`).
const FIREHOSE_TARGET_PREFIX: &str = "Firehose_20150804";
const FIREHOSE_SIGNING_NAME: &str = "firehose";

/// The `DeliveryStreamStatus` a stream accepts data in.
const ACTIVE: &str = "ACTIVE";

/// The `StackStatus` values the deploy and the update settle at.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The one-minute period an exported record covers ("For metrics with regular resolution, a
/// period can be as short as one minute (60 seconds) and must be a multiple of 60",
/// `MetricStat$Period`), which is the span one record summarises and so the bucket a phase's
/// publishes fall in.
const PERIOD_SECS: f64 = 60.0;

/// The unit every datapoint below is published with.
const UNIT: &str = "Count";

/// How often a stack operation is asked whether it has settled, and how often the destination is
/// asked whether a delivery has arrived.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a stack operation and a delivery are waited for before a row fails.
///
/// Both are WALL-CLOCK deadlines rather than counts of reads: the spans the services behind a
/// stack and the buffering behind a delivery choose take the time they take, so what is waited
/// for is the CONDITION, re-read in a poll loop. Generous, so a loaded machine is slower than
/// those spans without either rule being any different; all they are for is that the wait
/// terminates.
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);
const DELIVERY_DEADLINE: Duration = Duration::from_secs(90);

/// The trust policy every row's role carries: Firehose assumes the role its destination names,
/// and a metric stream exports under the role it is configured with.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"#,
    r#"["firehose.amazonaws.com","streams.metrics.cloudwatch.amazonaws.com"]},"#,
    r#""Action":"sts:AssumeRole"}]}"#
);

/// The role's permissions: the record write the metric stream makes on the delivery stream
/// ("This IAM role must include the following permissions: firehose:PutRecord
/// firehose:PutRecordBatch" — `PutMetricStreamInput$RoleArn`) and the object write the delivery
/// itself makes on the bucket.
const ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["firehose:PutRecord","firehose:PutRecordBatch","s3:PutObject"],"Resource":"*"}]}"#
);

/// The key prefix every row's destination declares, so the records are read from where the
/// declaration puts them.
const PREFIX: &str = "metric-stream/";

/// One metric a row publishes in both phases: the namespace and metric name it is published
/// under, the value it carries before the update and the value it carries after it.
type Published = (&'static str, &'static str, f64, f64);

/// One `(namespace, metric name)` pair, as a record reports it.
type Metric = (&'static str, &'static str);

/// One row: the stack it deploys, the metric-stream filter its template declares before and
/// after the update, what it publishes in each phase, and which of those publishes the
/// destination must hold a record for in each phase.
struct Row {
    case: &'static str,
    /// The stack's name, which every other name the row needs is derived from.
    stack: &'static str,
    /// The filter members the `AWS::CloudWatch::MetricStream` declares, as the template writes
    /// them, before the update and after it.
    before: fn() -> Value,
    after: fn() -> Value,
    /// Everything the row publishes, in both phases.
    published: &'static [Published],
    /// The pairs a record must arrive for in the first phase, and in the second; every other
    /// published pair is one no record of that phase may arrive for.
    selected_before: &'static [Metric],
    selected_after: &'static [Metric],
}

impl Row {
    /// The resource names the row's template declares, each derived from the stack's own name so
    /// no two rows can collide even where they share an emulator.
    fn bucket(&self) -> String {
        format!("{}-bucket", self.stack)
    }

    fn role(&self) -> String {
        format!("{}-role", self.stack)
    }

    fn delivery_stream(&self) -> String {
        format!("{}-firehose", self.stack)
    }

    fn metric_stream(&self) -> String {
        format!("{}-stream", self.stack)
    }

    /// Whether this phase's filter selects `metric`.
    fn selects(&self, phase: Phase, metric: Metric) -> bool {
        self.selected(phase).contains(&metric)
    }

    /// The pairs the phase's filter selects.
    fn selected(&self, phase: Phase) -> &'static [Metric] {
        match phase {
            Phase::Before => self.selected_before,
            Phase::After => self.selected_after,
        }
    }
}

/// Which side of the update a publish and its records belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Before,
    After,
}

impl Phase {
    /// How the phase is named in a failure.
    fn named(self) -> &'static str {
        match self {
            Self::Before => "before the update",
            Self::After => "after the update",
        }
    }
}

/// The template a row deploys, with `filters` as the metric stream's own filter declaration.
///
/// Everything outside that one declaration is identical between the row's two templates, so the
/// update the row makes asks for exactly the filter change and nothing else.
fn template(row: &Row, filters: &Value) -> String {
    let mut stream_properties = json!({
        "Name": row.metric_stream(),
        "FirehoseArn": {"Fn::GetAtt": ["MetricsStream", "Arn"]},
        "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
        "OutputFormat": "json",
    });
    let declared = filters.as_object().cloned().unwrap_or_default();
    if let Some(properties) = stream_properties.as_object_mut() {
        for (member, value) in declared {
            properties.insert(member, value);
        }
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a metric-stream export pipeline whose filter a stack update rewrites",
        "Resources": {
            "DeliveryRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": row.role(),
                    "AssumeRolePolicyDocument": TRUST_POLICY,
                    "Policies": [{"PolicyName": "export", "PolicyDocument": ROLE_POLICY}],
                },
            },
            "MetricsBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": row.bucket()},
            },
            "MetricsStream": {
                "Type": "AWS::KinesisFirehose::DeliveryStream",
                "Properties": {
                    "DeliveryStreamName": row.delivery_stream(),
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
                        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
                        "Prefix": PREFIX,
                        "CompressionFormat": "UNCOMPRESSED",
                        // The SHORTEST interval the hint admits
                        // (`BufferingHints$IntervalInSeconds`, published range 0 to 900):
                        // "buffer incoming data for the specified period of time, in seconds,
                        // before delivering it to the destination" is the wait a delivery is
                        // held for, and this row asserts what the destination holds rather than
                        // waiting out an interval a consumer might configure.
                        "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
                    },
                },
            },
            "MetricStream": {
                "Type": "AWS::CloudWatch::MetricStream",
                "Properties": stream_properties,
            },
        },
        "Outputs": {
            "StreamName": {"Value": {"Ref": "MetricStream"}},
            "DeliveryStreamName": {"Value": {"Ref": "MetricsStream"}},
            "BucketName": {"Value": {"Ref": "MetricsBucket"}},
        },
    })
    .to_string()
}

const ROWS: &[Row] = &[
    Row {
        case: "an include filter widened by a second namespace",
        stack: "msfu-widen-namespace",
        before: || json!({"IncludeFilters": [{"Namespace": "Custom/Alpha"}]}),
        after: || {
            json!({
                "IncludeFilters": [{"Namespace": "Custom/Alpha"}, {"Namespace": "Custom/Beta"}],
            })
        },
        published: &[
            ("Custom/Alpha", "Orders", 7.0, 11.0),
            ("Custom/Beta", "Carts", 3.0, 5.0),
        ],
        selected_before: &[("Custom/Alpha", "Orders")],
        selected_after: &[("Custom/Alpha", "Orders"), ("Custom/Beta", "Carts")],
    },
    Row {
        case: "an include filter widened from one metric name to the whole namespace",
        stack: "msfu-widen-metric-names",
        before: || {
            json!({
                "IncludeFilters": [{"Namespace": "Custom/Alpha", "MetricNames": ["Orders"]}],
            })
        },
        after: || json!({"IncludeFilters": [{"Namespace": "Custom/Alpha"}]}),
        published: &[
            ("Custom/Alpha", "Orders", 2.0, 4.0),
            ("Custom/Alpha", "Returns", 8.0, 6.0),
        ],
        selected_before: &[("Custom/Alpha", "Orders")],
        selected_after: &[("Custom/Alpha", "Orders"), ("Custom/Alpha", "Returns")],
    },
    Row {
        case: "an include filter narrowed to one of the two namespaces it named",
        stack: "msfu-narrow-namespace",
        before: || {
            json!({
                "IncludeFilters": [{"Namespace": "Custom/Alpha"}, {"Namespace": "Custom/Beta"}],
            })
        },
        after: || json!({"IncludeFilters": [{"Namespace": "Custom/Beta"}]}),
        published: &[
            ("Custom/Alpha", "Orders", 9.0, 1.0),
            ("Custom/Beta", "Carts", 4.0, 12.0),
        ],
        selected_before: &[("Custom/Alpha", "Orders"), ("Custom/Beta", "Carts")],
        selected_after: &[("Custom/Beta", "Carts")],
    },
    Row {
        case: "an exclude filter replaced by an include filter naming what it had excluded",
        stack: "msfu-exclude-to-include",
        before: || json!({"ExcludeFilters": [{"Namespace": "Custom/Beta"}]}),
        after: || json!({"IncludeFilters": [{"Namespace": "Custom/Beta"}]}),
        published: &[
            ("Custom/Alpha", "Orders", 6.0, 10.0),
            ("Custom/Beta", "Carts", 13.0, 2.0),
        ],
        selected_before: &[("Custom/Alpha", "Orders")],
        selected_after: &[("Custom/Beta", "Carts")],
    },
];

/// The emulator one row drives: the five services the flow spans, on ONE engine.
fn emulator() -> MultiServiceRouter {
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

/// One request to the service `signing_name` serves; answers its status and body bytes.
async fn send(
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

/// One CloudFormation action over its awsQuery wire. The form components are encoded by the test
/// crate's one awsQuery encoder, so a template body full of `{`, `"` and `&` survives it.
async fn cloudformation(
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

/// One JSON-RPC operation of a service that speaks one of the awsJson protocols, as its answer.
async fn json_rpc(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    content_type: &str,
    target: &str,
    request: &Value,
) -> Value {
    let (status, answered) = send(
        router,
        signing_name,
        "POST",
        "/",
        &[("content-type", content_type), ("x-amz-target", target)],
        request.to_string().into_bytes(),
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {target}, got {status} with {text}"
    );
    if text.trim().is_empty() {
        return Value::Null;
    }
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {case:?}: {target} must answer JSON, got {text}: {error}"))
}

/// Publish one datapoint, stamped inside the phase's own period.
async fn put_metric_data(
    router: &MultiServiceRouter,
    case: &str,
    namespace: &str,
    metric_name: &str,
    value: f64,
    stamp: f64,
) {
    json_rpc(
        router,
        case,
        CLOUDWATCH_SIGNING_NAME,
        "application/x-amz-json-1.0",
        &format!("{CLOUDWATCH_TARGET_PREFIX}.PutMetricData"),
        &json!({
            "Namespace": namespace,
            "MetricData": [{
                "MetricName": metric_name,
                "Value": value,
                "Unit": UNIT,
                "Timestamp": stamp,
            }],
        }),
    )
    .await;
}

/// The `DeliveryStreamStatus` Amazon Data Firehose reports for the stream the stack deployed.
async fn delivery_stream_status(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let described = json_rpc(
        router,
        case,
        FIREHOSE_SIGNING_NAME,
        "application/x-amz-json-1.1",
        &format!("{FIREHOSE_TARGET_PREFIX}.DescribeDeliveryStream"),
        &json!({"DeliveryStreamName": name}),
    )
    .await;
    described
        .get("DeliveryStreamDescription")
        .and_then(|description| description.get("DeliveryStreamStatus"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The one stack the row deployed, as the response body.
async fn describe_stack(router: &MultiServiceRouter, row: &Row) -> String {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", row.stack)]).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}",
        row.case
    );
    body
}

/// The `DescribeStacks` body of the first SETTLED observation of the row's stack — the one the
/// rules are stated about, since it is the observation a client acts on.
async fn settled(router: &MultiServiceRouter, row: &Row) -> String {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = describe_stack(router, row).await;
        let status = element(&described, "StackStatus");
        if status.ends_with("_COMPLETE") || status.ends_with("_FAILED") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: the stack operation must settle, it is still {status} with body \
             {described}",
            row.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Deploy the row's pipeline, or apply its update, and answer the first settled observation.
async fn deploy(router: &MultiServiceRouter, row: &Row, action: &str, filters: &Value) -> String {
    let (status, body) = cloudformation(
        router,
        action,
        &[
            ("StackName", row.stack),
            ("TemplateBody", &template(row, filters)),
            // The template declares the IAM role the export and the delivery are made under, by
            // name, so the request acknowledges its named IAM resources.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from {action}, got {status} with body {body}",
        row.case
    );
    settled(router, row).await
}

/// Every key the row's bucket holds, read through `ListObjectsV2`.
///
/// The listing is read in one page and each key is used in a request path as written: a row
/// delivers a handful of objects, and a delivered object's key is the destination's prefix plus
/// the automatic time segments plus the object's own name, which carry nothing a path has to
/// escape.
async fn keys(router: &MultiServiceRouter, row: &Row) -> Vec<String> {
    let bucket = row.bucket();
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{bucket}?list-type=2"),
        &[],
        Vec::new(),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from ListObjectsV2 on {bucket}, got {status} with {text}",
        row.case
    );
    let mut found = elements(&text, "Key");
    found.sort();
    found
}

/// Every record the row's bucket holds, in key order: a delivered object is the records of one
/// buffer concatenated, each record one JSON document on its own line.
async fn delivered(router: &MultiServiceRouter, row: &Row) -> Vec<Value> {
    let bucket = row.bucket();
    let mut records = Vec::new();
    for key in keys(router, row).await {
        assert!(
            key.starts_with(PREFIX),
            "case {:?}: a delivered object must be written under {PREFIX:?}, got {key:?}",
            row.case
        );
        let (status, body) = send(
            router,
            "s3",
            "GET",
            &format!("/{bucket}/{key}"),
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
                    "case {:?}: every line of {key:?} must be a JSON record, got {line:?}: {error}",
                    row.case
                )
            });
            records.push(record);
        }
    }
    records
}

/// The records of ONE phase — the ones stamped with the instant that phase's period opens at —
/// once at least `want` of them have arrived, or a failure naming the row and the phase.
async fn delivered_in(
    router: &MultiServiceRouter,
    row: &Row,
    phase: Phase,
    period: f64,
    want: usize,
) -> Vec<Value> {
    let stamped = (period * 1_000.0) as i64;
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let records = delivered(router, row).await;
        let of_phase: Vec<Value> = records
            .iter()
            .filter(|record| record.get("timestamp").and_then(Value::as_i64) == Some(stamped))
            .cloned()
            .collect();
        if of_phase.len() >= want {
            return of_phase;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected at least {want} record(s) stamped {stamped} ({}) in {}, got \
             {of_phase:?} of {records:?}",
            row.case,
            phase.named(),
            row.bucket()
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

/// The instant the period `back` periods before the current one opens at, as the epoch seconds
/// the awsJson protocols carry a timestamp as.
///
/// Read ONCE per row and every expectation built from it, so a period boundary the clock crosses
/// while a row waits for a delivery cannot move what a record is expected to be stamped with.
fn period_start(back: f64) -> f64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    (now / PERIOD_SECS).floor() * PERIOD_SECS - back * PERIOD_SECS
}

/// The pairs a set of records reports, as the set a phase's expectation is compared against.
fn pairs_of(records: &[Value]) -> BTreeSet<(String, String)> {
    records.iter().map(metric_of).collect()
}

/// The pairs a phase expects, as the same set.
fn expected_pairs(selected: &[Metric]) -> BTreeSet<(String, String)> {
    selected
        .iter()
        .map(|(namespace, metric_name)| ((*namespace).to_string(), (*metric_name).to_string()))
        .collect()
}

/// Publish one phase's datapoints and assert the destination then holds a record for exactly the
/// pairs that phase's filter selects.
///
/// The publishes the filter does NOT select are made FIRST: an export is pushed into the delivery
/// stream from the publish that owes it, and a buffer is delivered in arrival order, so a record
/// this phase must not export would be in the destination by the time the ones it must export
/// are — which is what makes the absence below an assertion rather than a race.
async fn assert_phase(
    router: &MultiServiceRouter,
    row: &Row,
    phase: Phase,
    period: f64,
    stream_name: &str,
) {
    let stamp = period + PERIOD_SECS / 2.0;
    let case = row.case;
    let mut order: Vec<&Published> = row.published.iter().collect();
    order.sort_by_key(|published| row.selects(phase, (published.0, published.1)));
    for (namespace, metric_name, before, after) in order {
        let value = match phase {
            Phase::Before => *before,
            Phase::After => *after,
        };
        put_metric_data(router, case, namespace, metric_name, value, stamp).await;
    }

    let selected = row.selected(phase);
    let records = delivered_in(router, row, phase, period, selected.len()).await;
    let seen = pairs_of(&records);
    let expected = expected_pairs(selected);
    assert_eq!(
        seen,
        expected,
        "case {case:?}: {} the stream's filter selects exactly {expected:?}, got records for \
         {seen:?} in {records:?}",
        phase.named()
    );
    assert_eq!(
        records.len(),
        selected.len(),
        "case {case:?}: {} one selected metric of one period owes one record, got {records:?}",
        phase.named()
    );

    for (namespace, metric_name, before, after) in row.published {
        if !row.selects(phase, (*namespace, *metric_name)) {
            continue;
        }
        let value = match phase {
            Phase::Before => *before,
            Phase::After => *after,
        };
        let reported = ((*namespace).to_string(), (*metric_name).to_string());
        let record = records
            .iter()
            .find(|record| metric_of(record) == reported)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} the destination must hold a record for \
                     {namespace}/{metric_name}, got {records:?}",
                    phase.named()
                )
            });
        assert_eq!(
            text(record, "metric_stream_name"),
            stream_name,
            "case {case:?}: {} the record of {namespace}/{metric_name} must name the deployed \
             stream {stream_name:?}, got {record:?}",
            phase.named()
        );
        assert_eq!(
            text(record, "unit"),
            UNIT,
            "case {case:?}: {} the record of {namespace}/{metric_name} must report the unit the \
             datapoint was published with, got {record:?}",
            phase.named()
        );
        // "a metric stream always sends the `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT` statistics for
        // each metric that is streamed" (`PutMetricStream`), computed over the one datapoint the
        // phase published for this pair.
        for (name, expected_value) in [
            ("max", value),
            ("min", value),
            ("sum", value),
            ("count", 1.0),
        ] {
            assert_eq!(
                statistic(record, name),
                Some(expected_value),
                "case {case:?}: {} the record of {namespace}/{metric_name} must report \
                 {name}={expected_value}, got {record:?}",
                phase.named()
            );
        }
    }
}

#[tokio::test]
async fn a_stack_update_rewriting_a_metric_stream_filter_changes_what_the_pipeline_exports() {
    for row in ROWS {
        let case = row.case;
        let router = emulator();

        // The pipeline, deployed exactly as a client deploys it.
        let described = deploy(&router, row, "CreateStack", &(row.before)()).await;
        let stack_status = element(&described, "StackStatus");
        assert_eq!(
            stack_status, CREATE_COMPLETE,
            "case {case:?}: the pipeline's stack must complete, got {stack_status} with body \
             {described}"
        );
        let outputs = outputs_of(&described);
        let stream_name = outputs.get("StreamName").cloned().unwrap_or_default();
        assert_eq!(
            stream_name,
            row.metric_stream(),
            "case {case:?}: the stack must report the deployed metric stream, got {outputs:?}"
        );
        let delivery_stream = outputs.get("DeliveryStreamName").cloned().unwrap_or_default();
        assert_eq!(
            delivery_stream,
            row.delivery_stream(),
            "case {case:?}: the stack must report the deployed delivery stream, got {outputs:?}"
        );

        // Rule 1, asked the way a client asks it: ONE read of the stream, straight after the
        // first observation of the completed stack and with no polling of its own.
        let status = delivery_stream_status(&router, case, &delivery_stream).await;
        assert_eq!(
            status, ACTIVE,
            "case {case:?}: the completed stack's delivery stream {delivery_stream:?} must be \
             {ACTIVE}, got {status}"
        );

        // Rule 2, over the two phases. Each phase publishes into its own period, so the records
        // of one are told from the other's by the instant they are stamped with.
        let (first, second) = (period_start(2.0), period_start(1.0));
        assert_phase(&router, row, Phase::Before, first, &stream_name).await;

        let updated = deploy(&router, row, "UpdateStack", &(row.after)()).await;
        let stack_status = element(&updated, "StackStatus");
        assert_eq!(
            stack_status, UPDATE_COMPLETE,
            "case {case:?}: the filter update must complete, got {stack_status} with body \
             {updated}"
        );

        assert_phase(&router, row, Phase::After, second, &stream_name).await;
    }
}
