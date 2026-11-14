// SPDX-License-Identifier: CC0-1.0

//! The metric-stream EXPORT path, end to end over the wire: CloudWatch, Amazon Data Firehose
//! and Amazon S3 on one engine, exactly as the router mounts them.
//!
//! The rule under test: a metric stream in the `running` state EXPORTS every metric update its
//! filters select into the delivery stream its `FirehoseArn` names — "Metric streams can
//! automatically stream CloudWatch metrics to Amazon Web Services destinations, including
//! Amazon S3" (`PutMetricStream`) — as one record per metric per one-minute period, carrying
//! the statistics "a metric stream always sends … `MAX`, `MIN`, `SUM`, and `SAMPLECOUNT`" plus
//! whatever `StatisticsConfigurations` adds for that metric, and the records of ONE metric
//! arrive in the order its datapoints were published. Which metrics are selected is the three
//! choices the operation documents: every namespace, every namespace except the
//! `ExcludeFilters` ones, or only the `IncludeFilters` ones (narrowed per namespace by
//! `MetricStreamFilter$MetricNames`). A stopped stream exports nothing.
//!
//! Nothing here reads CloudWatch to check the export: the records are asserted where a client
//! sees them, in the destination bucket of the Firehose stream, read back through S3's own
//! `ListObjectsV2` / `GetObject`.
//!
//! The periods a case needs are reached through `MetricDatum$Timestamp` ("You can specify time
//! stamps that are as much as two weeks before the current date"), so a case covering three
//! consecutive periods costs no wall clock at all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::TARGET_PREFIX;

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// The Region every call below is signed for.
pub(crate) const REGION: &str = "us-east-1";

/// The account the emulator serves, as every other suite in this target spells it.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The role a metric stream delivers through, and the one the Firehose destination assumes.
/// No IAM service is mounted here, so this only names the role form each member documents.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/MetricStreamRole";

/// Long enough for a created Firehose stream to be `ACTIVE`, which is when it takes records:
/// that service's own clock-owed `CREATING` -> `ACTIVE` span, with room to spare.
const PAST_THE_FIREHOSE_TRANSITION: Duration = Duration::from_millis(3_400);

/// How long a delivery is waited for before a case fails: far longer than the Firehose
/// delivery span, so a slow machine does not fail a case the clock would have served.
pub(crate) const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How long a case waits before concluding that NOTHING was exported — the same budget a
/// positive case gets, so an absence is asserted after as long a wait as a delivery needs.
const SILENCE_WINDOW: Duration = Duration::from_secs(3);

/// How often the destination is polled while waiting for a delivery.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The one-minute period an exported record covers (`MetricStat$Period`: "a period can be as
/// short as one minute (60 seconds) and must be a multiple of 60").
pub(crate) const PERIOD_SECS: f64 = 60.0;

/// What a dispatched S3 request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

/// CloudWatch, Amazon Data Firehose and Amazon S3 on one shared engine.
///
/// Shared with every other suite that asserts an export where a client sees it: the pipeline a
/// metric stream delivers through is this one whatever output format the stream carries.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's concern, not this service's.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One `awsJson` RPC call against the service whose signing name is `signing_name`.
    async fn rpc(
        &self,
        signing_name: &str,
        content_type: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", content_type)
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", authorization(signing_name))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed: Value = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch operation, requiring a 200.
    pub(crate) async fn cloudwatch(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc(
                "monitoring",
                "application/x-amz-json-1.0",
                TARGET_PREFIX,
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One Firehose operation, requiring a 200.
    pub(crate) async fn firehose(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc(
                "firehose",
                "application/x-amz-json-1.1",
                "Firehose_20150804",
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One path-style Amazon S3 request.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            body,
        }
    }

    /// `PUT /<bucket>` — S3's `CreateBucket`, in the Region every call here is signed for.
    ///
    /// The create names NO `LocationConstraint`: the unspecified constraint stands for
    /// us-east-1, which is the Region this suite signs for, and naming it explicitly is what S3
    /// refuses.
    pub(crate) async fn create_bucket(&self, bucket: &str) {
        let answer = self.s3("PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            answer.status,
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// Every key `bucket` holds, read through `ListObjectsV2`.
    pub(crate) async fn keys(&self, bucket: &str) -> Vec<String> {
        let answer = self.s3("GET", &format!("/{bucket}?list-type=2"), b"").await;
        assert_eq!(
            answer.status,
            200,
            "list {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let mut keys: Vec<String> = elements(&answer.body)
            .into_iter()
            .filter(|(element, _)| element == "Key")
            .map(|(_, key)| key)
            .collect();
        keys.sort();
        keys
    }

    /// The bytes `bucket` holds at `key`, read through `GetObject`.
    pub(crate) async fn object(&self, bucket: &str, key: &str) -> Vec<u8> {
        let answer = self.s3("GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            answer.status,
            200,
            "get {bucket}/{key}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
        answer.body.to_vec()
    }

    /// The exported records every object in `bucket` holds, one list per object, in key order:
    /// a delivered object is the records of one buffer concatenated, and each record is one
    /// JSON document on its own line.
    async fn delivered_objects(&self, bucket: &str) -> Vec<Vec<Value>> {
        let mut objects = Vec::new();
        for key in self.keys(bucket).await {
            let body = self.object(bucket, &key).await;
            let text = String::from_utf8(body)
                .unwrap_or_else(|error| panic!("object {key} must be UTF-8 text: {error}"));
            let records: Vec<Value> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str(line).unwrap_or_else(|error| {
                        panic!("record {line:?} of object {key} must be JSON: {error}")
                    })
                })
                .collect();
            objects.push(records);
        }
        objects
    }

    /// Every exported record in `bucket`, in delivery order, once at least `want` of them
    /// have arrived — or a failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
    async fn delivered_records(&self, case: &str, bucket: &str, want: usize) -> Vec<Value> {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let records: Vec<Value> = self
                .delivered_objects(bucket)
                .await
                .into_iter()
                .flatten()
                .collect();
            if records.len() >= want {
                return records;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected at least {want} exported record(s) in {bucket}, got \
                 {records:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Every exported record in `bucket`, in delivery order, once one has arrived for each of
    /// `want` — or a failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
    ///
    /// The condition is the METRICS rather than a count: a destination of a stream whose filters
    /// admit the account's own usage namespace also holds the records of the call count this
    /// service publishes about its API, so a count could be satisfied by records of a metric the
    /// case is not waiting for.
    async fn delivered_metrics(
        &self,
        case: &str,
        bucket: &str,
        want: &[(&str, &str)],
    ) -> Vec<Value> {
        let wanted: Vec<(String, String)> = want
            .iter()
            .map(|(namespace, metric_name)| {
                ((*namespace).to_string(), (*metric_name).to_string())
            })
            .collect();
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let records: Vec<Value> = self
                .delivered_objects(bucket)
                .await
                .into_iter()
                .flatten()
                .collect();
            let arrived: Vec<(String, String)> = records.iter().map(metric_of).collect();
            if wanted.iter().all(|metric| arrived.contains(metric)) {
                return records;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected an exported record for each of {wanted:?} in {bucket}, \
                 got {records:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Every exported record in `bucket` after [`SILENCE_WINDOW`], for a case asserting what
    /// was NOT exported.
    async fn records_after_the_silence_window(&self, bucket: &str) -> Vec<Value> {
        tokio::time::sleep(SILENCE_WINDOW).await;
        self.delivered_objects(bucket)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// The pipeline every case stands up: the destination bucket, the Firehose delivery stream
    /// writing into it (waited past its `CREATING` span, since a put is accepted only then),
    /// and the metric stream `PutMetricStream` is given `configuration`.
    async fn given_pipeline(&self, bucket: &str, delivery_stream: &str, configuration: Value) {
        self.create_bucket(bucket).await;
        self.firehose(
            "CreateDeliveryStream",
            &json!({
                "DeliveryStreamName": delivery_stream,
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": format!("arn:aws:s3:::{bucket}"),
                    // The shortest wait Firehose's `IntervalInSeconds` admits (its published
                    // range is 0..900), so a case asserting that the metric stream's records
                    // reach the bucket does not wait out a buffering interval it is not about.
                    "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
                },
            }),
        )
        .await;
        tokio::time::sleep(PAST_THE_FIREHOSE_TRANSITION).await;
        self.cloudwatch("PutMetricStream", &configuration).await;
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key the
/// router claims a request by. Neither the key id nor the signature is a credential: this
/// harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Every element of an XML document as `(element name, text)` in document order, read with an
/// XML reader rather than by slicing the body.
fn elements(body: &[u8]) -> Vec<(String, String)> {
    let document = String::from_utf8_lossy(body).into_owned();
    let mut reader = Reader::from_str(&document);
    let mut read = Vec::new();
    let mut element = String::new();
    let mut current = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
            }
            Event::Text(text) => current.push_str(&text.xml10_content().unwrap_or_default()),
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                if closing == element {
                    read.push((closing, current.clone()));
                    current.clear();
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    read
}

/// A `PutMetricStream` request naming `filters` (the member and its value) and, where a row
/// asks for them, statistics configurations.
fn metric_stream_request(
    name: &str,
    delivery_stream: &str,
    filters: Option<(&str, Value)>,
    statistics: Option<Value>,
) -> Value {
    let mut request = json!({
        "Name": name,
        "FirehoseArn": format!("arn:aws:firehose:{REGION}:{ACCOUNT}:deliverystream/{delivery_stream}"),
        "RoleArn": ROLE_ARN,
        "OutputFormat": "json",
    });
    if let Some((member, value)) = filters {
        request[member] = value;
    }
    if let Some(statistics) = statistics {
        request["StatisticsConfigurations"] = statistics;
    }
    request
}

/// Publish one datapoint, stamped in the period `periods_back` periods before the current one.
async fn publish(
    emulator: &Emulator,
    namespace: &str,
    metric_name: &str,
    value: f64,
    periods_back: i64,
) {
    publish_datum(
        emulator,
        namespace,
        json!({
            "MetricName": metric_name,
            "Value": value,
            "Unit": "Count",
            "Timestamp": stamp(periods_back),
        }),
    )
    .await;
}

/// Publish one `MetricDatum` as written.
pub(crate) async fn publish_datum(emulator: &Emulator, namespace: &str, datum: Value) {
    emulator
        .cloudwatch(
            "PutMetricData",
            &json!({"Namespace": namespace, "MetricData": [datum]}),
        )
        .await;
}

/// A time stamp in the middle of the period `periods_back` periods before the one the clock is
/// in, as the epoch seconds the awsJson protocols carry a timestamp as.
///
/// The MIDDLE of the period keeps a row's placement independent of where in a period the suite
/// happens to run, and a period boundary crossed mid-case moves every stamp of the case
/// together.
pub(crate) fn stamp(periods_back: i64) -> f64 {
    period_of(periods_back) + PERIOD_SECS / 2.0
}

/// The instant the period `periods_back` periods before the current one opens at.
pub(crate) fn period_of(periods_back: i64) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    (now / PERIOD_SECS).floor() * PERIOD_SECS - (periods_back as f64) * PERIOD_SECS
}

/// A string member of a record.
fn text(record: &Value, member: &str) -> String {
    record
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A number member of a record's `value` object.
fn statistic(record: &Value, name: &str) -> Option<f64> {
    record.get("value")?.get(name)?.as_f64()
}

/// The `timestamp` a record carries, in the milliseconds the json record format stamps it with.
fn timestamp(record: &Value) -> i64 {
    record
        .get("timestamp")
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

/// The `(namespace, metric_name)` of a record.
fn metric_of(record: &Value) -> (String, String) {
    (text(record, "namespace"), text(record, "metric_name"))
}

/// One filter-selection case: what the stream was configured with, what was published, and
/// which of those publishes the destination must receive a record for.
struct SelectionRow {
    case: &'static str,
    /// The `PutMetricStream` member carrying the filters, and its value.
    filters: Option<(&'static str, Value)>,
    /// The metrics published, as `(namespace, metric name, value)`.
    published: Vec<(&'static str, &'static str, f64)>,
    /// The `(namespace, metric name)` pairs a record must arrive for.
    exported: Vec<(&'static str, &'static str)>,
    /// The `(namespace, metric name)` pairs no record may arrive for.
    withheld: Vec<(&'static str, &'static str)>,
}

/// The selection cases.
///
/// The rows differ along what the rule generalises over: which of the three documented choices
/// the stream was configured with (no filter, `IncludeFilters`, `ExcludeFilters`), whether a
/// filter narrows a namespace to named metrics, how many namespaces are published into, and
/// how many filters the list carries. Every row publishes something that must be exported
/// BESIDE anything it expects withheld, so no row can pass by exporting nothing at all.
fn selection_rows() -> Vec<SelectionRow> {
    vec![
        SelectionRow {
            case: "no filter at all streams every namespace",
            filters: None,
            published: vec![
                ("Custom/Alpha", "Latency", 11.0),
                ("Custom/Beta", "Errors", 12.0),
            ],
            exported: vec![("Custom/Alpha", "Latency"), ("Custom/Beta", "Errors")],
            withheld: vec![],
        },
        SelectionRow {
            case: "an include filter streams only the namespace it names",
            filters: Some((
                "IncludeFilters",
                json!([{"Namespace": "Custom/Alpha"}]),
            )),
            published: vec![
                ("Custom/Alpha", "Latency", 21.0),
                ("Custom/Beta", "Errors", 22.0),
            ],
            exported: vec![("Custom/Alpha", "Latency")],
            withheld: vec![("Custom/Beta", "Errors")],
        },
        SelectionRow {
            case: "two include filters stream both namespaces they name",
            filters: Some((
                "IncludeFilters",
                json!([{"Namespace": "Custom/Alpha"}, {"Namespace": "Custom/Beta"}]),
            )),
            published: vec![
                ("Custom/Alpha", "Latency", 31.0),
                ("Custom/Beta", "Errors", 32.0),
                ("Custom/Gamma", "Saturation", 33.0),
            ],
            exported: vec![("Custom/Alpha", "Latency"), ("Custom/Beta", "Errors")],
            withheld: vec![("Custom/Gamma", "Saturation")],
        },
        SelectionRow {
            case: "an include filter naming metric names streams only those metrics",
            filters: Some((
                "IncludeFilters",
                json!([{"Namespace": "Custom/Alpha", "MetricNames": ["Latency"]}]),
            )),
            published: vec![
                ("Custom/Alpha", "Latency", 41.0),
                ("Custom/Alpha", "Errors", 42.0),
            ],
            exported: vec![("Custom/Alpha", "Latency")],
            withheld: vec![("Custom/Alpha", "Errors")],
        },
        SelectionRow {
            case: "an exclude filter streams every namespace except the one it names",
            filters: Some((
                "ExcludeFilters",
                json!([{"Namespace": "Custom/Alpha"}]),
            )),
            published: vec![
                ("Custom/Alpha", "Latency", 51.0),
                ("Custom/Beta", "Errors", 52.0),
            ],
            exported: vec![("Custom/Beta", "Errors")],
            withheld: vec![("Custom/Alpha", "Latency")],
        },
        SelectionRow {
            case: "an exclude filter naming metric names excludes only those metrics",
            filters: Some((
                "ExcludeFilters",
                json!([{"Namespace": "Custom/Alpha", "MetricNames": ["Errors"]}]),
            )),
            published: vec![
                ("Custom/Alpha", "Latency", 61.0),
                ("Custom/Alpha", "Errors", 62.0),
            ],
            exported: vec![("Custom/Alpha", "Latency")],
            withheld: vec![("Custom/Alpha", "Errors")],
        },
    ]
}

/// RULE: a running stream exports a record for every metric update its filters select, and for
/// no other.
#[tokio::test]
async fn a_running_stream_exports_the_metric_updates_its_filters_select() {
    for row in selection_rows() {
        let emulator = Emulator::new();
        emulator
            .given_pipeline(
                "selection-sink",
                "selection-stream",
                metric_stream_request("selection", "selection-stream", row.filters.clone(), None),
            )
            .await;
        for (namespace, metric_name, value) in &row.published {
            publish(&emulator, namespace, metric_name, *value, 1).await;
        }
        let records = emulator
            .delivered_metrics(row.case, "selection-sink", &row.exported)
            .await;
        let arrived: Vec<(String, String)> = records.iter().map(metric_of).collect();
        for (namespace, metric_name) in &row.exported {
            let wanted = ((*namespace).to_string(), (*metric_name).to_string());
            assert!(
                arrived.contains(&wanted),
                "case {:?}: expected a record for {wanted:?}, got {arrived:?}",
                row.case
            );
        }
        // Whatever the stream withholds stays withheld for as long as a delivery is waited
        // for, so the absence is not merely a record that had not arrived yet.
        let settled = emulator
            .records_after_the_silence_window("selection-sink")
            .await;
        let settled_metrics: Vec<(String, String)> = settled.iter().map(metric_of).collect();
        for (namespace, metric_name) in &row.withheld {
            let unwanted = ((*namespace).to_string(), (*metric_name).to_string());
            assert!(
                !settled_metrics.contains(&unwanted),
                "case {:?}: expected no record for {unwanted:?}, got {settled_metrics:?}",
                row.case
            );
        }
    }
}

/// RULE: one record reports the metric's identity, the period it covers and the statistics a
/// stream always sends, computed over the datapoints of that period.
#[tokio::test]
async fn an_exported_record_reports_the_period_it_covers() {
    let emulator = Emulator::new();
    emulator
        .given_pipeline(
            "record-sink",
            "record-stream",
            metric_stream_request(
                "record",
                "record-stream",
                Some(("IncludeFilters", json!([{"Namespace": "Custom/Record"}]))),
                None,
            ),
        )
        .await;
    // Two observations of ONE period, so the record's statistics are the fold over both and
    // not either one of them.
    let period = period_of(1);
    publish_datum(
        &emulator,
        "Custom/Record",
        json!({
            "MetricName": "Latency",
            "Dimensions": [{"Name": "Service", "Value": "checkout"}],
            "Values": [20.0, 40.0],
            "Unit": "Milliseconds",
            "Timestamp": stamp(1),
        }),
    )
    .await;
    let records = emulator
        .delivered_records("one record per period", "record-sink", 1)
        .await;
    assert_eq!(
        records.len(),
        1,
        "one metric update of one period must export one record, got {records:?}"
    );
    let record = &records[0];
    for (member, expected) in [
        ("metric_stream_name", "record"),
        ("account_id", ACCOUNT),
        ("region", REGION),
        ("namespace", "Custom/Record"),
        ("metric_name", "Latency"),
        ("unit", "Milliseconds"),
    ] {
        assert_eq!(
            text(record, member),
            expected,
            "case {member:?}: expected {expected:?}, got {record}"
        );
    }
    assert_eq!(
        record.get("dimensions"),
        Some(&json!({"Service": "checkout"})),
        "the record must carry the metric's dimensions, got {record}"
    );
    assert_eq!(
        timestamp(record),
        (period * 1_000.0) as i64,
        "the record must be stamped at the instant its period opens, got {record}"
    );
    for (name, expected) in [
        ("sum", 60.0),
        ("count", 2.0),
        ("min", 20.0),
        ("max", 40.0),
    ] {
        assert_eq!(
            statistic(record, name),
            Some(expected),
            "case {name:?}: expected {expected}, got {record}"
        );
    }
}

/// RULE: the records of ONE metric arrive in the order its datapoints were published — a
/// record per period, their sums following the publish order and their timestamps
/// non-decreasing.
///
/// This is the recorded guarantee without its wall clock: the three periods are reached through
/// `MetricDatum$Timestamp` rather than by waiting out three minutes.
#[tokio::test]
async fn the_records_of_one_metric_arrive_in_publish_order() {
    let emulator = Emulator::new();
    emulator
        .given_pipeline(
            "ordering-sink",
            "ordering-stream",
            metric_stream_request(
                "ordering",
                "ordering-stream",
                Some(("IncludeFilters", json!([{"Namespace": "Custom/Ordering"}]))),
                None,
            ),
        )
        .await;
    // One publish per period, oldest first: 100 two periods back, 200 one period back, 300 in
    // the period the clock is in.
    let published: [(i64, f64); 3] = [(2, 100.0), (1, 200.0), (0, 300.0)];
    // The period each row is published INTO, read off the clock ONCE per row and used both for
    // the datapoint's stamp and for what the record is expected to carry: a period boundary
    // crossed while the pipeline delivers moves `period_of` on, but not the stamp a record
    // already carries, so re-reading it at assertion time would compare two different periods.
    let mut periods: Vec<f64> = Vec::new();
    for (periods_back, value) in published {
        let period = period_of(periods_back);
        periods.push(period);
        publish_datum(
            &emulator,
            "Custom/Ordering",
            json!({
                "MetricName": "Streamed",
                "Value": value,
                "Unit": "Count",
                // The MIDDLE of the period, for the reason `stamp` records.
                "Timestamp": period + PERIOD_SECS / 2.0,
            }),
        )
        .await;
    }
    let records = emulator
        .delivered_records("three periods of one metric", "ordering-sink", 3)
        .await;
    assert_eq!(
        records.len(),
        3,
        "three publishes in three periods must export three records, got {records:?}"
    );
    let sums: Vec<Option<f64>> = records
        .iter()
        .map(|record| statistic(record, "sum"))
        .collect();
    assert_eq!(
        sums,
        published
            .iter()
            .map(|(_, value)| Some(*value))
            .collect::<Vec<Option<f64>>>(),
        "the exported sums must follow the publish order, got {records:?}"
    );
    let timestamps: Vec<i64> = records.iter().map(timestamp).collect();
    assert_eq!(
        timestamps,
        periods
            .iter()
            .map(|period| (period * 1_000.0) as i64)
            .collect::<Vec<i64>>(),
        "each record must be stamped at its own period, got {timestamps:?}"
    );
    assert!(
        timestamps.windows(2).all(|pair| pair[0] <= pair[1]),
        "the record timestamps must be non-decreasing, got {timestamps:?}"
    );
    // And the pipeline kept that order within each delivered buffer, which is where a consumer
    // reads it.
    for object in emulator.delivered_objects("ordering-sink").await {
        let stamps: Vec<i64> = object.iter().map(timestamp).collect();
        assert!(
            stamps.windows(2).all(|pair| pair[0] <= pair[1]),
            "the records of one delivered object must be in chronological order, got {stamps:?}"
        );
    }
}

/// RULE: a stream sends the additional statistics its `StatisticsConfigurations` name for the
/// metrics they name, beside the four every streamed metric gets — and nothing extra for a
/// metric no configuration names.
#[tokio::test]
async fn additional_statistics_are_exported_for_the_metrics_they_name() {
    let emulator = Emulator::new();
    emulator
        .given_pipeline(
            "statistics-sink",
            "statistics-stream",
            metric_stream_request(
                "statistics",
                "statistics-stream",
                Some(("IncludeFilters", json!([{"Namespace": "Custom/Statistics"}]))),
                Some(json!([{
                    "IncludeMetrics": [
                        {"Namespace": "Custom/Statistics", "MetricName": "Latency"},
                    ],
                    "AdditionalStatistics": ["p99", "IQM"],
                }])),
            ),
        )
        .await;
    // Ten published values, so the percentile is computed over a distribution rather than over
    // one opaque aggregate: by nearest rank the 99th percentile of these is the largest of
    // them, and the interquartile mean is the mean of the middle half.
    let values: Vec<f64> = (1..=10).map(f64::from).collect();
    publish_datum(
        &emulator,
        "Custom/Statistics",
        json!({
            "MetricName": "Latency",
            "Values": values,
            "Unit": "Count",
            "Timestamp": stamp(1),
        }),
    )
    .await;
    publish(&emulator, "Custom/Statistics", "Errors", 7.0, 1).await;
    let records = emulator
        .delivered_records("additional statistics", "statistics-sink", 2)
        .await;
    let by_metric: BTreeMap<String, &Value> = records
        .iter()
        .map(|record| (text(record, "metric_name"), record))
        .collect();
    let named = by_metric
        .get("Latency")
        .expect("the configured metric must be exported");
    assert_eq!(
        statistic(named, "p99"),
        Some(10.0),
        "the named metric must carry p99, got {named}"
    );
    assert_eq!(
        statistic(named, "IQM"),
        Some(5.5),
        "the named metric must carry the interquartile mean, got {named}"
    );
    let other = by_metric
        .get("Errors")
        .expect("the other metric must be exported too");
    for name in ["p99", "IQM"] {
        assert_eq!(
            statistic(other, name),
            None,
            "case {name:?}: a metric no configuration names must carry no additional \
             statistic, got {other}"
        );
    }
    // The four a stream always sends are there whichever metric it is.
    for record in records.iter() {
        for name in ["max", "min", "sum", "count"] {
            assert!(
                statistic(record, name).is_some(),
                "case {name:?}: every record carries the statistics a stream always sends, got \
                 {record}"
            );
        }
    }
}

/// RULE: only a stream in the `running` state exports — a stopped one exports nothing, and
/// exports again once it is started.
#[tokio::test]
async fn a_stopped_stream_exports_nothing_until_it_is_started() {
    let emulator = Emulator::new();
    emulator
        .given_pipeline(
            "state-sink",
            "state-stream",
            metric_stream_request(
                "state",
                "state-stream",
                Some(("IncludeFilters", json!([{"Namespace": "Custom/State"}]))),
                None,
            ),
        )
        .await;
    emulator
        .cloudwatch("StopMetricStreams", &json!({"Names": ["state"]}))
        .await;
    publish(&emulator, "Custom/State", "WhileStopped", 1.0, 1).await;
    let settled = emulator.records_after_the_silence_window("state-sink").await;
    assert!(
        settled.is_empty(),
        "a stopped stream must export nothing, got {settled:?}"
    );
    emulator
        .cloudwatch("StartMetricStreams", &json!({"Names": ["state"]}))
        .await;
    publish(&emulator, "Custom/State", "WhileRunning", 2.0, 1).await;
    let records = emulator
        .delivered_records("started again", "state-sink", 1)
        .await;
    let arrived: Vec<String> = records
        .iter()
        .map(|record| text(record, "metric_name"))
        .collect();
    assert_eq!(
        arrived,
        vec!["WhileRunning".to_string()],
        "a started stream must export the updates published after it was started, and only \
         those, got {records:?}"
    );
}
