// SPDX-License-Identifier: CC0-1.0

//! The WHOLE lifecycle of a monitoring estate a stack deploys — two
//! `AWS::CloudWatch::MetricStream` resources over one Amazon Data Firehose destination — driven
//! the way a client drives it: deploy, read each stream back, walk the listing to find the one
//! that EXCLUDES a namespace, stop them, empty the destination, destroy the stack, and read what
//! is left.
//!
//! The rule under test: **every call of that lifecycle TERMINATES with an answer or a modelled
//! refusal, and each one answers from the estate's own state.** A control-plane call that
//! neither answers nor fails is indistinguishable from an outage: a client scripting metric
//! streams has nothing to branch on, so the lifecycle is driven here end to end, each phase
//! waiting for its CONDITION in a poll loop with a generous deadline rather than for a span.
//!
//! What each phase has to answer, with the facts the operations state:
//!
//! * The deploy — a CREATE change set, waited for, then executed, which is the shape an
//!   `aws cloudformation deploy` takes — reaches `CREATE_COMPLETE`.
//! * `GetMetricStream` reports, for each declared stream, the filter half its declaration
//!   carried with its `MetricNames` in the order the template wrote them ("If you omit this
//!   parameter, all metrics in the namespace are included or excluded",
//!   `MetricStreamFilter$MetricNames`), the `FirehoseArn` of the delivery stream the same stack
//!   created, `State` `running` ("When you use `PutMetricStream` to create a new metric stream,
//!   the stream is created in the `running` state", `PutMetricStream`) — and NOT the other
//!   filter member at all, which a stream that declared one half does not carry.
//! * `ListMetricStreams` walks to a LAST page: "The token that marks the start of the next batch
//!   of returned results" (`ListMetricStreamsOutput$NextToken`) is handed back only while a page
//!   is left, so a client paging the estate finishes — and the entries carry no filters, which
//!   is why finding the excluding stream needs the read above.
//! * `StopMetricStreams` leaves each `State` `stopped` ("The possible values are `running` and
//!   `stopped`", `GetMetricStreamOutput$State`), and the destination can then be emptied and
//!   stays empty.
//! * `DeleteStack` takes the streams with it: `GetMetricStream` answers the modelled
//!   `ResourceNotFoundException` (404) with no message of its own, and the stack is no longer
//!   resolvable by name — `DescribeStackResources` refuses with `ValidationError` and the
//!   service's own "does not exist" sentence.
//!
//! Only a cross-service suite can ask this, and it runs over the WHOLE service set the binary
//! serves (`aws_emulator_router::build_router`), on one engine: the flow spans CloudFormation
//! for the deploy and the destroy, IAM for the role the export and the delivery are made under,
//! CloudWatch for the streams, Firehose for the delivery stream and Amazon S3 for the
//! destination, and each of those calls the next in process.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES its streams
//! or leaves the names to CloudFormation, which filter half each stream declares (an include
//! half over two namespaces, an exclude half narrowed to five metric names, an exclude half over
//! a whole namespace, no filter at all), whether the destination declares buffering hints at all
//! or the shortest interval the hint admits, whether its objects are compressed and whether it
//! writes under a key prefix — so no row can pass by delivering nothing where another row
//! delivers, and neither the naming nor the filter half decides whether the lifecycle answers.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_metric_stream_exports_to_s3::{
    cloudformation, send, CLOUDWATCH_SIGNING_NAME, CLOUDWATCH_TARGET_PREFIX,
};
use crate::query_wire::{element, elements, form_encode, outputs_of};

/// Assembling the whole router costs more stack than a test thread's default, so it is
/// assembled on a thread sized like the binary's own boot thread
/// (`crate::boot_serve::BOOT_STACK_SIZE`, which this matches).
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// How long a phase's condition is waited for before the row fails, and how often it is
/// re-read. Generous against every span these services choose: what a phase waits for is the
/// CONDITION, re-read in a loop, so a loaded host cannot fail a row the clock would have served.
const CONDITION_DEADLINE: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long the destination is watched, after it has been emptied, to show that a stopped estate
/// delivers nothing further. A negative observation has no condition to wait for, so it is made
/// repeatedly over this window rather than once after a wait.
///
/// Longer than the window a destination's buffer can be held for — the records a stopped stream
/// had already handed over are written at the next flush, and a destination declaring no
/// `BufferingHints` is one whose window this emulator picks — so the silence a teardown waits
/// for covers a pending flush rather than ending before it.
const SILENCE_WINDOW: Duration = Duration::from_secs(10);

/// The XML namespace an Amazon S3 multi-object delete carries.
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

/// The two filter members, which are mutually exclusive on one stream: a declaration carrying
/// one of them is reported carrying that one and NOT the other
/// (`PutMetricStreamInput$IncludeFilters` / `$ExcludeFilters`).
const INCLUDE_FILTERS: &str = "IncludeFilters";
const EXCLUDE_FILTERS: &str = "ExcludeFilters";

/// The metric names one row's exclude half narrows its namespace to, in an order no sort
/// reproduces — the shape of filter a client writes to drop a few of one namespace's metrics
/// and stream everything else. They are metric names of `AWS/EC2`
/// (`NetworkPacketsIn`, `CPUCreditBalance`, `NetworkIn`, `DiskWriteBytes`, `CPUUtilization`).
const EXCLUDED_METRIC_NAMES: [&str; 5] = [
    "NetworkPacketsIn",
    "CPUCreditBalance",
    "NetworkIn",
    "DiskWriteBytes",
    "CPUUtilization",
];

/// One metric stream a row's template declares.
struct StreamDeclaration {
    /// The logical id it is declared under, which is also the output the stack publishes its
    /// name as.
    logical_id: &'static str,
    /// The `Name` the template states, or `None` for a stream CloudFormation names.
    name: Option<&'static str>,
    /// The filter half it declares — `(member, value)` — or `None` for a stream declaring
    /// neither, which streams every namespace of the account.
    filters: Option<fn() -> (&'static str, Value)>,
}

impl StreamDeclaration {
    /// The filter member this declaration does NOT carry, which the read must not report.
    fn absent_member(&self) -> Option<&'static str> {
        match self.filters.map(|filters| filters().0) {
            Some(INCLUDE_FILTERS) => Some(EXCLUDE_FILTERS),
            Some(EXCLUDE_FILTERS) => Some(INCLUDE_FILTERS),
            _ => None,
        }
    }
}

/// One row: the estate a stack deploys and the handles its lifecycle is driven by.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    change_set: &'static str,
    bucket: &'static str,
    role: &'static str,
    firehose: &'static str,
    /// The key `Prefix` the destination declares, where it declares one.
    prefix: Option<&'static str>,
    /// `CompressionFormat` the destination declares.
    compression: &'static str,
    /// The `BufferingHints` the destination declares, or `None` for a destination that declares
    /// none at all and so buffers for Firehose's own documented default.
    hints: Option<fn() -> Value>,
    streams: &'static [StreamDeclaration],
}

const ROWS: &[Row] = &[
    Row {
        case: "two named streams, an include half over two namespaces beside an exclude half \
               narrowed to five metric names, over a destination declaring no buffering hints",
        stack_name: "monstream-named",
        change_set: "monstream-named-changes",
        bucket: "monstream-named-metrics",
        role: "monstream-named-role",
        firehose: "monstream-named-firehose",
        prefix: Some("metrics/"),
        compression: "UNCOMPRESSED",
        hints: None,
        streams: &[
            StreamDeclaration {
                logical_id: "IncludeStream",
                name: Some("monstream-named-include"),
                filters: Some(|| {
                    (
                        INCLUDE_FILTERS,
                        json!([{"Namespace": "AWS/EC2"}, {"Namespace": "AWS/Lambda"}]),
                    )
                }),
            },
            StreamDeclaration {
                logical_id: "ExcludeStream",
                name: Some("monstream-named-exclude"),
                filters: Some(|| {
                    (
                        EXCLUDE_FILTERS,
                        json!([{"Namespace": "AWS/EC2", "MetricNames": EXCLUDED_METRIC_NAMES}]),
                    )
                }),
            },
        ],
    },
    Row {
        case: "two streams CloudFormation names, one unfiltered and one excluding a whole \
               namespace, over a compressed destination buffering for the shortest interval",
        stack_name: "monstream-generated",
        change_set: "monstream-generated-changes",
        bucket: "monstream-generated-metrics",
        role: "monstream-generated-role",
        firehose: "monstream-generated-firehose",
        prefix: None,
        compression: "GZIP",
        // The shortest interval the hint admits (`BufferingHints$IntervalInSeconds`, published
        // range 0 to 900), so this row's destination really does receive the objects the stream
        // exports while the lifecycle runs and its emptying is asserted over them.
        hints: Some(|| json!({"SizeInMBs": 1, "IntervalInSeconds": 0})),
        streams: &[
            StreamDeclaration {
                logical_id: "EveryNamespaceStream",
                name: None,
                filters: None,
            },
            StreamDeclaration {
                logical_id: "ExcludeStream",
                name: None,
                filters: Some(|| {
                    (
                        EXCLUDE_FILTERS,
                        json!([{"Namespace": "Custom/Monitoring/Noise"}]),
                    )
                }),
            },
        ],
    },
];

/// The whole emulator, on one engine, exactly as the binary assembles it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router
}

/// The template one row deploys: the delivery role, the destination bucket, the delivery stream
/// writing into it, and the row's metric streams over that one destination.
fn template(row: &Row) -> String {
    let mut destination = json!({
        "BucketARN": {"Fn::GetAtt": ["MetricsBucket", "Arn"]},
        "RoleARN": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
        "CompressionFormat": row.compression,
    });
    if let Some(hints) = row.hints {
        destination["BufferingHints"] = hints();
    }
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
        "FirehoseArn": {"Value": {"Fn::GetAtt": ["MetricsFirehose", "Arn"]}},
    });
    for stream in row.streams {
        let mut properties = json!({
            "FirehoseArn": {"Fn::GetAtt": ["MetricsFirehose", "Arn"]},
            "RoleArn": {"Fn::GetAtt": ["DeliveryRole", "Arn"]},
            "OutputFormat": "json",
        });
        if let Some(name) = stream.name {
            properties["Name"] = json!(name);
        }
        if let Some(filters) = stream.filters {
            let (member, declared) = filters();
            properties[member] = declared;
        }
        resources[stream.logical_id] = json!({
            "Type": "AWS::CloudWatch::MetricStream",
            "Properties": properties,
        });
        // "Ref returns the name of the metric stream", which is the handle every CloudWatch
        // call about it takes — and the only handle a client has for a stream it did not name.
        outputs[stream.logical_id] = json!({"Value": {"Ref": stream.logical_id}});
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a monitoring estate of metric streams over one Firehose destination",
        "Resources": resources,
        "Outputs": outputs,
    })
    .to_string()
}

/// One CloudWatch operation over its awsJson1_0 wire, as its status and its parsed body: several
/// phases below drive the service into its modelled refusal and read what it answered.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
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

/// Every key `bucket` holds, read through `ListObjectsV2` and following the listing's own
/// continuation so a destination holding more than one page is read whole.
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

/// Remove `held` from `bucket` with the ONE multi-object delete a client empties a bucket with,
/// requiring every listed key to be reported deleted.
async fn delete_objects(router: &MultiServiceRouter, case: &str, bucket: &str, held: &[String]) {
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
    let deleted = elements(&text, "Key");
    assert_eq!(
        deleted.len(),
        held.len(),
        "case {case:?}: expected every listed key to be reported deleted, listed {held:?} and got \
         {deleted:?}"
    );
}

/// Empty `bucket` the way a teardown does — list, delete what was listed, look again — and
/// answer every key that was ever removed, requiring the destination to REACH empty.
///
/// A destination something keeps writing into never reaches empty, so the loop is bounded by a
/// deadline and the row fails rather than hanging.
async fn drain(router: &MultiServiceRouter, case: &str, bucket: &str) -> BTreeSet<String> {
    let deadline = Instant::now() + CONDITION_DEADLINE;
    let mut emptied: BTreeSet<String> = BTreeSet::new();
    loop {
        let held = keys(router, case, bucket).await;
        if held.is_empty() {
            return emptied;
        }
        delete_objects(router, case, bucket, &held).await;
        emptied.extend(held);
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {bucket} never reached empty — after removing {} object(s) it still \
             lists objects, so a stopped estate is still delivering",
            emptied.len()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Empty `bucket` and keep emptying it until it STAYS empty for a whole silence window, and
/// answer every key that was removed.
///
/// What a teardown depends on is that the delivery into the destination is FINITE once every
/// stream feeding it is stopped: the records those streams had already handed to the delivery
/// stream are written at its next flush — "buffer incoming data for the specified period of
/// time, in seconds, before delivering it to the destination"
/// (`BufferingHints$IntervalInSeconds`) — and no further ones are made, so the loop reaches a
/// destination that is empty and stays empty. A destination something keeps writing into never
/// does, and the deadline is what fails the row rather than hanging it.
async fn quiesce(router: &MultiServiceRouter, case: &str, bucket: &str) -> BTreeSet<String> {
    let deadline = Instant::now() + CONDITION_DEADLINE;
    let mut emptied: BTreeSet<String> = BTreeSet::new();
    loop {
        emptied.extend(drain(router, case, bucket).await);
        // Watch the emptied destination: an object that arrives at any point in the window is
        // removed and the window started again, so the row passes only once the delivery has
        // really stopped.
        let until = Instant::now() + SILENCE_WINDOW;
        let mut arrived: Vec<String> = Vec::new();
        while Instant::now() < until {
            tokio::time::sleep(POLL_INTERVAL).await;
            let held = keys(router, case, bucket).await;
            if !held.is_empty() {
                arrived = held;
                break;
            }
        }
        if arrived.is_empty() {
            return emptied;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {bucket} never settled empty — {arrived:?} kept arriving after every \
             stream feeding it was stopped"
        );
    }
}

/// The row's stack as `DescribeStacks` reports it, with its status.
async fn describe_stack(router: &MultiServiceRouter, row: &Row) -> (u16, String) {
    cloudformation(router, "DescribeStacks", &[("StackName", row.stack_name)]).await
}

/// Deploy the row's estate the way a client deploys one — a CREATE change set, waited for, then
/// executed — and answer the `DescribeStacks` body of its first SETTLED observation.
async fn deploy(router: &MultiServiceRouter, row: &Row) -> String {
    let (status, created) = cloudformation(
        router,
        "CreateChangeSet",
        &[
            ("StackName", row.stack_name),
            ("ChangeSetName", row.change_set),
            ("ChangeSetType", "CREATE"),
            ("TemplateBody", &template(row)),
            // Every row's template declares the IAM role the export and the delivery are made
            // under, by name, so the request acknowledges the template's named IAM resources.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from CreateChangeSet, got {status} with {created}",
        row.case
    );

    // The change set a client executes is the one that has finished being computed, which is
    // what it waits for before executing it.
    let deadline = Instant::now() + CONDITION_DEADLINE;
    loop {
        let (status, body) = cloudformation(
            router,
            "DescribeChangeSet",
            &[
                ("StackName", row.stack_name),
                ("ChangeSetName", row.change_set),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DescribeChangeSet, got {status} with {body}",
            row.case
        );
        let reported = element(&body, "Status");
        if reported == "CREATE_COMPLETE" {
            break;
        }
        assert!(
            !reported.ends_with("_FAILED"),
            "case {:?}: expected the change set to be computed, got {reported:?} in {body}",
            row.case
        );
        assert!(
            Instant::now() < deadline,
            "case {:?}: the change set never settled, last status {reported:?}",
            row.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let (status, executed) = cloudformation(
        router,
        "ExecuteChangeSet",
        &[
            ("StackName", row.stack_name),
            ("ChangeSetName", row.change_set),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from ExecuteChangeSet, got {status} with {executed}",
        row.case
    );

    let deadline = Instant::now() + CONDITION_DEADLINE;
    loop {
        let (status, body) = describe_stack(router, row).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DescribeStacks, got {status} with {body}",
            row.case
        );
        let reported = element(&body, "StackStatus");
        if reported.ends_with("_COMPLETE") || reported.ends_with("_FAILED") {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: the deployment never settled, last status {reported:?}",
            row.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every stream `ListMetricStreams` reports, walked a page at a time: the walk follows the token
/// each page hands back and is bounded, so a listing that never ends fails the row instead of
/// hanging it.
///
/// `page_size` is the `MaxResults` each request asks for, or `None` for the request a client
/// paging with no page size of its own makes.
async fn walk_metric_streams(
    router: &MultiServiceRouter,
    row: &Row,
    page_size: Option<u32>,
) -> BTreeSet<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    // One page per stream, plus the page that ends the walk, plus margin: a walk that has not
    // ended by then is not ending.
    let pages = row.streams.len() + 8;
    for _ in 0..pages {
        let mut request = Value::Object(serde_json::Map::new());
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(next) = &token {
            request["NextToken"] = json!(next);
        }
        let (status, listed) = cloudwatch(router, "ListMetricStreams", &request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from ListMetricStreams, got {status} with {listed}",
            row.case
        );
        for entry in listed
            .get("Entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = entry.get("Name").and_then(Value::as_str).unwrap_or_default();
            assert!(
                found.insert(name.to_string()),
                "case {:?}: expected each page to report a stream once, got {name:?} twice",
                row.case
            );
        }
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => token = Some(next.to_string()),
            _ => return found,
        }
    }
    panic!(
        "case {:?}: the listing kept handing back a token after {pages} pages, so a client \
         walking it never finishes; it reported {found:?}",
        row.case
    );
}

/// The whole lifecycle of one row, every phase waiting for its own condition.
async fn lifecycle(row: &Row) {
    let router = emulator();
    let described = deploy(&router, row).await;
    assert_eq!(
        element(&described, "StackStatus"),
        "CREATE_COMPLETE",
        "case {:?}: expected the monitoring estate to deploy, got {described}",
        row.case
    );
    let outputs = outputs_of(&described);
    let firehose_arn = outputs
        .get("FirehoseArn")
        .cloned()
        .unwrap_or_else(|| panic!("case {:?}: no FirehoseArn output in {described}", row.case));
    let bucket = outputs
        .get("BucketName")
        .cloned()
        .unwrap_or_else(|| panic!("case {:?}: no BucketName output in {described}", row.case));
    assert_eq!(
        bucket, row.bucket,
        "case {:?}: expected the stack to report its destination bucket, got {outputs:?}",
        row.case
    );

    // The handle a client holds for each stream is the one the stack published for it, whether
    // the template named the stream or CloudFormation did.
    let names: Vec<String> = row
        .streams
        .iter()
        .map(|stream| {
            let published = outputs.get(stream.logical_id).cloned().unwrap_or_else(|| {
                panic!(
                    "case {:?}: no {} output in {described}",
                    row.case, stream.logical_id
                )
            });
            if let Some(declared) = stream.name {
                assert_eq!(
                    published, declared,
                    "case {:?}: expected {} to be published under its declared name, got \
                     {published:?}",
                    row.case, stream.logical_id
                );
            }
            assert!(
                !published.is_empty(),
                "case {:?}: expected {} to publish a name, got {published:?}",
                row.case,
                stream.logical_id
            );
            published
        })
        .collect();

    // Each stream reads back as its declaration asked for, over the delivery stream the same
    // stack created, running.
    for (stream, name) in row.streams.iter().zip(&names) {
        let (status, reported) = cloudwatch(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetMetricStream for {name:?}, got {status} with \
             {reported}",
            row.case
        );
        if let Some(filters) = stream.filters {
            let (member, declared) = filters();
            assert_eq!(
                reported.get(member),
                Some(&declared),
                "case {:?}: expected {name:?} to report {member} as the template declared it, got \
                 {reported}",
                row.case
            );
        }
        if let Some(absent) = stream.absent_member() {
            assert_eq!(
                reported.get(absent),
                None,
                "case {:?}: expected {name:?} to carry no {absent} member at all, got {reported}",
                row.case
            );
        }
        assert_eq!(
            (
                reported.get("State").and_then(Value::as_str),
                reported.get("FirehoseArn").and_then(Value::as_str),
            ),
            (Some("running"), Some(firehose_arn.as_str())),
            "case {:?}: expected {name:?} to be running over the stack's delivery stream, got \
             {reported}",
            row.case
        );
    }

    // The listing a client walks to FIND a stream, both a page at a time and with no page size
    // of its own: either walk terminates and reports the estate.
    let declared: BTreeSet<String> = names.iter().cloned().collect();
    for page_size in [Some(1), None] {
        let walked = walk_metric_streams(&router, row, page_size).await;
        assert_eq!(
            walked, declared,
            "case {:?}: expected the walk with MaxResults {page_size:?} to report every stream of \
             the estate once, got {walked:?}",
            row.case
        );
    }

    // Stopping every stream is what quiesces the delivery into the destination.
    let (status, stopped) =
        cloudwatch(&router, "StopMetricStreams", &json!({"Names": names})).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from StopMetricStreams, got {status} with {stopped}",
        row.case
    );
    for name in &names {
        let (status, reported) = cloudwatch(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            (status, reported.get("State").and_then(Value::as_str)),
            (200, Some("stopped")),
            "case {:?}: expected {name:?} to be stopped, got {status} with {reported}",
            row.case
        );
    }

    // The destination a stopped estate leaves behind can be emptied, and it stays empty — which
    // is what a teardown depends on: a bucket something keeps writing into is never emptied, so
    // the destroy that follows never runs.
    let emptied = quiesce(&router, row.case, &bucket).await;
    assert!(
        emptied.iter().all(|key| !key.is_empty()),
        "case {:?}: every removed key names an object, got {emptied:?}",
        row.case
    );

    // And the destroy takes the streams with it.
    let (status, deleted) =
        cloudformation(&router, "DeleteStack", &[("StackName", row.stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from DeleteStack, got {status} with {deleted}",
        row.case
    );
    let deadline = Instant::now() + CONDITION_DEADLINE;
    loop {
        let (status, body) = describe_stack(&router, row).await;
        if status != 200 {
            break;
        }
        let reported = element(&body, "StackStatus");
        assert!(
            !reported.ends_with("DELETE_FAILED"),
            "case {:?}: expected the emptied estate to be deletable, got {body}",
            row.case
        );
        assert!(
            Instant::now() < deadline,
            "case {:?}: the destroy never finished, last status {reported:?}",
            row.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // What a client reads after the destroy: the streams are gone, each answering the modelled
    // not-found by its code alone, and the stack is no longer resolvable by name.
    for name in &names {
        let (status, gone) = cloudwatch(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            (status, cloudwatch_error_code(&gone).as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: expected {name:?} to be gone with its stack, got {status} with {gone}",
            row.case
        );
        assert_eq!(
            gone.get("Message").or_else(|| gone.get("message")),
            None,
            "case {:?}: expected the not-found for {name:?} to carry no message, got {gone}",
            row.case
        );
    }
    let (status, refused) = cloudformation(
        &router,
        "DescribeStackResources",
        &[("StackName", row.stack_name)],
    )
    .await;
    assert_eq!(
        (
            status,
            element(&refused, "Code"),
            element(&refused, "Message")
        ),
        (
            400,
            "ValidationError".to_string(),
            format!("Stack with id {} does not exist", row.stack_name)
        ),
        "case {:?}: expected the destroyed stack to be unresolvable by name, got {status} with \
         {refused}",
        row.case
    );
}

#[test]
fn a_monitoring_estates_whole_metric_stream_lifecycle_answers() {
    std::thread::Builder::new()
        .name("monitoring-metric-streams".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a runtime for the lifecycle");
            runtime.block_on(async {
                for row in ROWS {
                    lifecycle(row).await;
                }
            });
        })
        .expect("spawn the assembling thread")
        .join()
        .expect("every row's lifecycle answers");
}
