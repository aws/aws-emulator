// SPDX-License-Identifier: CC0-1.0

//! The Firehose-to-Amazon-S3 data plane, driven over the wire.
//!
//! The rule under test: a stream's buffered records are DELIVERED to the Amazon S3
//! location its current destination configuration names — "Firehose buffers records
//! before delivering them to the destination" (`PutRecord`) — so once the buffer is due
//! an object holding those records is readable out of that bucket, under the prefix the
//! configuration computes (`Prefix` with its automatic `YYYY/MM/DD/HH` expansion, or the
//! partition keys an enabled `DynamicPartitioningConfiguration` derives, and
//! `ErrorOutputPrefix` for a record whose processing failed), encoded as
//! `CompressionFormat` asks. A record a put REFUSES is never delivered, and an
//! `UpdateDestination` that repoints `BucketARN` sends every later delivery to the new
//! bucket while the old one stops receiving.
//!
//! Both services are mounted on ONE shared engine, exactly as the router mounts them, so
//! what is exercised is the hand-off between the two: the Firehose calls as `POST /` with
//! `X-Amz-Target`, the destination reads as the path-style S3 requests a client sends
//! (`GET /<bucket>?list-type=2`, `GET /<bucket>/<key>`).

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, REGION, ROLE_ARN};

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// How long a delivery is waited for before the case fails: far longer than the
/// emulator's delivery span, so a slow machine does not fail a case the clock would have
/// served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// What a dispatched request answered.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: bytes::Bytes,
    /// The response's own headers, which is where Amazon S3 reports what it keeps out of
    /// the body — the encryption an object was stored under, among others.
    pub(crate) headers: http::HeaderMap,
}

impl Answer {
    /// The value of the response header `name`, or `None` when the response carries none.
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// Firehose and Amazon S3 on one shared engine.
///
/// `pub(crate)`, like the single-service harness of `delivery_stream_lifecycle`, so a
/// sibling suite needing BOTH services reuses it instead of standing up a second one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The ONE engine every mounted service is built from, kept so a suite can read the
    /// store of a service whose own read path for what it holds is not served yet.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // An Amazon OpenSearch Service destination indexes its documents into a domain of
        // that service, so the suite that drives such a destination needs it mounted on
        // the same engine.
        router.register_service(
            service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // A `KinesisStreamAsSource` stream is fed by a Kinesis data stream, so the suite
        // that drives that source needs that service mounted on the same engine too.
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        // A destination converting its data format reads the column information of the Glue
        // table its `SchemaConfiguration` binds out of the Data Catalog, so the suite that
        // drives such a destination needs the Glue service mounted here as well.
        router.register_service(
            service_com_amazonaws_glue_awsglue::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// The shared engine, for a read of a service's own store.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Call one Firehose operation; answer its HTTP status and JSON body.
    pub(crate) async fn firehose(&self, operation: &str, body: Value) -> (u16, Value) {
        self.json_rpc("firehose", "Firehose_20150804", operation, body)
            .await
    }

    /// Call one operation of an `awsJson1_1` service mounted here: `service` is the routing
    /// key its requests are signed for and `target` the prefix of its `X-Amz-Target`.
    ///
    /// The one request path for every such service, so a suite driving the Kinesis data
    /// stream a Firehose stream reads from sends its calls exactly as the Firehose ones go.
    pub(crate) async fn json_rpc(
        &self,
        service: &str,
        target: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .header("authorization", authorization(service))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one Firehose operation and require it to succeed.
    pub(crate) async fn firehose_ok(&self, operation: &str, body: Value) -> Value {
        let (status, answered) = self.firehose(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {answered:?}");
        answered
    }

    /// One path-style Amazon S3 request, with `body` as the request's own bytes.
    async fn s3_with_body(&self, method: &str, path: &str, body: &[u8]) -> Answer {
        self.signed("s3", method, path, body).await
    }

    /// One request to the service `service` signs as, with `body` as its own bytes.
    pub(crate) async fn signed(
        &self,
        service: &str,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization(service))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            body,
            headers: parts.headers,
        }
    }

    /// One path-style Amazon S3 request.
    pub(crate) async fn s3(&self, method: &str, path: &str) -> Answer {
        self.s3_with_body(method, path, b"").await
    }

    /// `PUT /<bucket>` — S3's `CreateBucket`.
    ///
    /// The create names its `LocationConstraint`: a create that names none can only mean
    /// us-east-1, the Region the unspecified constraint stands for, so S3 refuses one sent
    /// to any other Region's endpoint and this harness signs for [`REGION`].
    pub(crate) async fn create_bucket(&self, bucket: &str) {
        let answer = self
            .s3_with_body(
                "PUT",
                &format!("/{bucket}"),
                format!(
                    "<CreateBucketConfiguration \
                     xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                     <LocationConstraint>{REGION}</LocationConstraint>\
                     </CreateBucketConfiguration>"
                )
                .as_bytes(),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
    }

    /// Every key `bucket` holds, read through `ListObjectsV2`.
    async fn keys(&self, bucket: &str) -> Vec<String> {
        self.keys_under(bucket, "").await
    }

    /// The bytes `bucket` holds at `key`, read through `GetObject`.
    pub(crate) async fn object(&self, bucket: &str, key: &str) -> Vec<u8> {
        let answer = self.s3("GET", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            answer.status,
            200,
            "get {bucket}/{key}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
        answer.body.to_vec()
    }

    /// Create a stream whose only destination is `member`: `config`, and wait for it to
    /// be `ACTIVE` — a put is accepted only then.
    pub(crate) async fn create_active_stream(&self, stream: &str, member: &str, config: Value) {
        self.create_active_stream_typed(stream, None, member, config)
            .await;
    }

    /// The same, naming the `DeliveryStreamType` the create asks for. `None` leaves the
    /// member out, which is the same source: a stream a provider application writes to
    /// directly ("`DirectPut`: Provider applications access the Firehose stream directly"
    /// — `CreateDeliveryStreamInput$DeliveryStreamType`).
    async fn create_active_stream_typed(
        &self,
        stream: &str,
        stream_type: Option<&str>,
        member: &str,
        config: Value,
    ) {
        let mut request = json!({ "DeliveryStreamName": stream });
        if let Some(stream_type) = stream_type {
            request["DeliveryStreamType"] = json!(stream_type);
        }
        request[member] = config;
        self.firehose_ok("CreateDeliveryStream", request).await;
        tokio::time::sleep(PAST_THE_TRANSITION).await;
    }

    /// `PutRecordBatch` with one record per entry of `records`.
    pub(crate) async fn put_batch(&self, stream: &str, records: &[Vec<u8>]) -> (u16, Value) {
        let entries: Vec<Value> = records
            .iter()
            .map(|data| json!({ "Data": base64(data) }))
            .collect();
        self.firehose(
            "PutRecordBatch",
            json!({ "DeliveryStreamName": stream, "Records": entries }),
        )
        .await
    }

    /// `PutRecord` with `data` as the one record — what a producer writing its records
    /// one at a time calls.
    pub(crate) async fn put_record(&self, stream: &str, data: &[u8]) -> (u16, Value) {
        self.firehose(
            "PutRecord",
            json!({
                "DeliveryStreamName": stream,
                "Record": { "Data": base64(data) },
            }),
        )
        .await
    }

    /// Every key under `prefix`, read through a `ListObjectsV2` that names NO delimiter —
    /// the recursive read a client makes for a prefix whose objects sit under further
    /// segments of their own. An empty `prefix` lists the whole bucket.
    pub(crate) async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        let answer = self
            .s3(
                "GET",
                &format!("/{bucket}?list-type=2&prefix={}", query_value(prefix)),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "list {bucket} under {prefix:?}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
        elements(&answer.body)
            .into_iter()
            .filter(|(element, _)| element == "Key")
            .map(|(_, key)| key)
            .collect()
    }

    /// What a `/`-delimited `ListObjectsV2` at `prefix` reports: the keys it lists AT that
    /// level, and how many entries the page holds in all — `KeyCount`, which counts each
    /// rolled-up common prefix as one.
    async fn delimited_listing(&self, bucket: &str, prefix: &str) -> (Vec<String>, usize) {
        let answer = self
            .s3(
                "GET",
                &format!(
                    "/{bucket}?list-type=2&delimiter=/&prefix={}",
                    query_value(prefix)
                ),
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "delimited list {bucket} at {prefix:?}: got {}",
            String::from_utf8_lossy(&answer.body)
        );
        let read = elements(&answer.body);
        let keys: Vec<String> = read
            .iter()
            .filter(|(element, _)| element == "Key")
            .map(|(_, key)| key.clone())
            .collect();
        let entries = read
            .iter()
            .find(|(element, _)| element == "KeyCount")
            .and_then(|(_, count)| count.parse().ok())
            .unwrap_or(0);
        (keys, entries)
    }

    /// Every key under `bucket` once the bucket holds at least `want` of them, or a
    /// failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
    pub(crate) async fn delivered_keys(&self, case: &str, bucket: &str, want: usize) -> Vec<String> {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let keys = self.keys(bucket).await;
            if keys.len() >= want {
                return keys;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected at least {want} delivered object(s) in {bucket}, \
                 got {keys:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The bytes of every object `bucket` holds, keyed by key.
    pub(crate) async fn delivered_bodies(&self, bucket: &str) -> BTreeMap<String, Vec<u8>> {
        let mut bodies = BTreeMap::new();
        for key in self.keys(bucket).await {
            let body = self.object(bucket, &key).await;
            bodies.insert(key, body);
        }
        bodies
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// `value` as a query-string value, for the reads that scope a listing to a prefix.
///
/// The harness interpolates it as written, so this is where a value that would have to be
/// encoded first fails loudly rather than silently listing something else: the prefixes
/// read here are object-key prefixes made of the unreserved characters and the delimiter.
fn query_value(value: &str) -> &str {
    assert!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte)),
        "a prefix read through the query string must need no encoding, got {value:?}"
    );
    value
}

/// `data` as the base64 the `awsJson1_1` wire carries a blob as.
pub(crate) fn base64(data: &[u8]) -> String {
    aws_smithy_types::base64::encode(data)
}

/// Whether `haystack` holds `needle` anywhere in it — what a suite asks to see whether a
/// record's own bytes reached an object.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Every element of an XML document as `(element name, text)` in document order, read with
/// an XML reader rather than by slicing the body.
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
            Event::GeneralRef(reference) => {
                current.push_str(&reference.decode().unwrap_or_default());
            }
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

/// A `CompressionFormat` value, and how a client reads an object written in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compression {
    Uncompressed,
    Gzip,
    Zip,
    Zstd,
    Snappy,
    HadoopSnappy,
}

impl Compression {
    /// The `CompressionFormat` value, or `None` to leave the member out of the
    /// configuration entirely ("If no value is specified, the default is UNCOMPRESSED").
    fn wire(self) -> Option<&'static str> {
        match self {
            Self::Uncompressed => None,
            Self::Gzip => Some("GZIP"),
            Self::Zip => Some("ZIP"),
            Self::Zstd => Some("ZSTD"),
            Self::Snappy => Some("Snappy"),
            Self::HadoopSnappy => Some("HADOOP_SNAPPY"),
        }
    }

    /// The default file extension an object in this format carries.
    fn extension(self) -> &'static str {
        match self {
            Self::Gzip => ".gz",
            Self::Zip => ".zip",
            Self::Zstd => ".zst",
            Self::Uncompressed | Self::Snappy | Self::HadoopSnappy => "",
        }
    }

    /// The bytes a delivered object holds, decoded with the maintained decoder for the
    /// format — which is what makes the assertion "the records are in there" a statement
    /// about the FORMAT and not about this emulator's own encoder.
    fn decode(self, body: &[u8]) -> Vec<u8> {
        match self {
            Self::Uncompressed => body.to_vec(),
            Self::Gzip => {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(body)
                    .read_to_end(&mut decoded)
                    .expect("a GZIP object must decode");
                decoded
            }
            Self::Zip => {
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(body.to_vec()))
                    .expect("a ZIP object must open");
                let mut decoded = Vec::new();
                archive
                    .by_index(0)
                    .expect("a ZIP object must hold an entry")
                    .read_to_end(&mut decoded)
                    .expect("a ZIP entry must read");
                decoded
            }
            Self::Zstd => {
                let mut decoded = Vec::new();
                ruzstd::decoding::StreamingDecoder::new(body)
                    .expect("a ZSTD object must open as a Zstandard frame")
                    .read_to_end(&mut decoded)
                    .expect("a ZSTD object must decode");
                decoded
            }
            Self::Snappy => {
                let mut decoded = Vec::new();
                snap::read::FrameDecoder::new(body)
                    .read_to_end(&mut decoded)
                    .expect("a Snappy object must decode");
                decoded
            }
            Self::HadoopSnappy => {
                // The Hadoop container: the stream's uncompressed length, then one block
                // as its compressed length followed by raw Snappy.
                assert!(
                    body.len() > 8,
                    "a HADOOP_SNAPPY object carries its two length fields"
                );
                snap::raw::Decoder::new()
                    .decompress_vec(&body[8..])
                    .expect("a HADOOP_SNAPPY block must decode")
            }
        }
    }
}

/// One delivery case: a stream with one destination, the records put into it, and what the
/// object delivered for them looks like.
struct DeliveryRow {
    case: &'static str,
    /// The `CreateDeliveryStream` member carrying the destination.
    member: &'static str,
    /// The destination configuration, without its `CompressionFormat` (the row's
    /// [`Compression`] supplies that) — `{bucket}` stands for the row's own bucket ARN.
    config: Value,
    compression: Compression,
    /// What every delivered key starts with, before the automatic time prefix.
    key_prefix: &'static str,
    /// Whether the key carries the automatic `YYYY/MM/DD/HH` prefix after `key_prefix`.
    automatic_time_prefix: bool,
    /// The extension the key ends with; `None` for the format's own default.
    file_extension: Option<&'static str>,
    /// The records put, in order.
    records: Vec<&'static str>,
    /// What the delivered object's decoded bytes are: the records concatenated, with this
    /// separator between them.
    separator: &'static str,
}

/// An S3 destination configuration naming `bucket`.
///
/// Its `BufferingHints` ask for the shortest wait `IntervalInSeconds` admits (its published
/// minimum is 0), because what every case here asserts is WHERE a delivered object lands
/// and WHAT it holds; how long a buffer is held for the configured interval is asserted by
/// the `buffering_hints` cases instead.
fn s3_destination(bucket: &str, prefix: Option<&str>) -> Value {
    let mut config = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": format!("arn:aws:s3:::{bucket}"),
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
    });
    if let Some(prefix) = prefix {
        config["Prefix"] = json!(prefix);
    }
    config
}

/// The delivery cases.
///
/// The rows differ along what the rule generalises over: which destination member carries
/// the Amazon S3 location (a plain destination, an extended one, and an Amazon Redshift
/// destination whose `S3Configuration` every record passes through), whether a custom
/// `Prefix` is configured and whether it carries an expression of its own, the
/// `CompressionFormat` the body is encoded with (every value `CompressionFormat` admits),
/// a `FileExtension` overriding the format's own, the `SizeInMBs` of the `BufferingHints`
/// the stream was created with (each published bound), the record count, and whether the
/// destination's processing asks for a delimiter between records.
fn delivery_rows() -> Vec<DeliveryRow> {
    vec![
        DeliveryRow {
            case: "a plain Amazon S3 destination with no prefix",
            member: "S3DestinationConfiguration",
            config: s3_destination("{bucket}", None),
            compression: Compression::Uncompressed,
            key_prefix: "",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["one"],
            separator: "",
        },
        DeliveryRow {
            case: "a plain destination under a custom prefix, three records in one buffer",
            member: "S3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("q/")),
            compression: Compression::Uncompressed,
            key_prefix: "q/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["first", "second", "third"],
            separator: "",
        },
        DeliveryRow {
            case: "an extended destination compressing with GZIP",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("gz/")),
            compression: Compression::Gzip,
            key_prefix: "gz/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["compress", "me"],
            separator: "",
        },
        DeliveryRow {
            case: "an extended destination compressing with ZIP",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("zipped/")),
            compression: Compression::Zip,
            key_prefix: "zipped/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["archive this"],
            separator: "",
        },
        DeliveryRow {
            case: "an extended destination compressing with ZSTD",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("zstd/")),
            compression: Compression::Zstd,
            key_prefix: "zstd/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["zstandard", "payload"],
            separator: "",
        },
        DeliveryRow {
            case: "an extended destination compressing with Snappy",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("snap/")),
            compression: Compression::Snappy,
            key_prefix: "snap/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["snappy payload"],
            separator: "",
        },
        DeliveryRow {
            case: "an extended destination compressing with HADOOP_SNAPPY",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("hsnap/")),
            compression: Compression::HadoopSnappy,
            key_prefix: "hsnap/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["hadoop snappy payload"],
            separator: "",
        },
        DeliveryRow {
            case: "a FileExtension overriding the default extension",
            member: "ExtendedS3DestinationConfiguration",
            config: {
                let mut config = s3_destination("{bucket}", Some("ext/"));
                config["FileExtension"] = json!(".jsonl");
                config
            },
            compression: Compression::Gzip,
            key_prefix: "ext/",
            automatic_time_prefix: true,
            file_extension: Some(".jsonl"),
            records: vec!["{\"a\":1}"],
            separator: "",
        },
        DeliveryRow {
            case: "the smallest buffering hints the model admits",
            member: "ExtendedS3DestinationConfiguration",
            config: {
                let mut config = s3_destination("{bucket}", Some("small/"));
                config["BufferingHints"] = json!({ "SizeInMBs": 1, "IntervalInSeconds": 0 });
                config
            },
            compression: Compression::Uncompressed,
            key_prefix: "small/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["tiny buffer"],
            separator: "",
        },
        DeliveryRow {
            // The largest `SizeInMBs` the model admits, with the interval that delivers
            // before it is reached: a buffer held for the LARGEST interval is not delivered
            // this soon at all, which is what the `buffering_hints` cases assert.
            case: "the largest SizeInMBs the model admits",
            member: "ExtendedS3DestinationConfiguration",
            config: {
                let mut config = s3_destination("{bucket}", Some("large/"));
                config["BufferingHints"] = json!({ "SizeInMBs": 128, "IntervalInSeconds": 0 });
                config
            },
            compression: Compression::Uncompressed,
            key_prefix: "large/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["large buffer"],
            separator: "",
        },
        DeliveryRow {
            case: "a destination whose processing appends a delimiter between records",
            member: "ExtendedS3DestinationConfiguration",
            config: {
                let mut config = s3_destination("{bucket}", Some("delim/"));
                config["ProcessingConfiguration"] = json!({
                    "Enabled": true,
                    "Processors": [{ "Type": "AppendDelimiterToRecord" }],
                });
                config
            },
            compression: Compression::Uncompressed,
            key_prefix: "delim/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["line one", "line two"],
            separator: "\n",
        },
        DeliveryRow {
            case: "a prefix carrying a timestamp expression, which replaces the automatic one",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("stamped/!{timestamp:yyyy}/")),
            compression: Compression::Uncompressed,
            key_prefix: "stamped/",
            automatic_time_prefix: false,
            file_extension: None,
            records: vec!["stamped"],
            separator: "",
        },
        DeliveryRow {
            case: "a prefix whose expression is never closed, which is not an expression",
            member: "ExtendedS3DestinationConfiguration",
            config: s3_destination("{bucket}", Some("raw/!{unclosed")),
            compression: Compression::Uncompressed,
            key_prefix: "raw/!{unclosed",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["malformed prefix"],
            separator: "",
        },
        DeliveryRow {
            case: "an Amazon Redshift destination, whose S3 configuration every record \
                   passes through",
            member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL": "jdbc:redshift://warehouse.123456789012.us-west-2.redshift.amazonaws.com:5439/dev",
                "CopyCommand": { "DataTableName": "events" },
                "Username": "loader",
                "Password": "loader-secret",
                "S3Configuration": s3_destination("{bucket}", Some("redshift/")),
            }),
            compression: Compression::Uncompressed,
            key_prefix: "redshift/",
            automatic_time_prefix: true,
            file_extension: None,
            records: vec!["warehouse row"],
            separator: "",
        },
    ]
}

/// Whether `key` carries a `YYYY/MM/DD/HH/` segment right after `prefix`.
///
/// `pub(crate)` so a sibling suite reading the same delivered keys — the source-fed streams
/// of `kinesis_source_ingestion` — judges them by this one reading.
pub(crate) fn carries_time_prefix(key: &str, prefix: &str) -> bool {
    let rest = match key.strip_prefix(prefix) {
        Some(rest) => rest,
        None => return false,
    };
    let fields: Vec<&str> = rest.split('/').take(4).collect();
    fields.len() == 4
        && fields[0].len() == 4
        && fields[1].len() == 2
        && fields[2].len() == 2
        && fields[3].len() == 2
        && fields.iter().all(|field| {
            field.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[tokio::test]
async fn a_streams_buffer_is_delivered_as_an_object_in_the_destination_bucket() {
    for (ordinal, row) in delivery_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("delivery-bucket-{ordinal}");
        let stream = format!("delivery-stream-{ordinal}");
        emulator.create_bucket(&bucket).await;

        // The row's destination, with its own bucket and compression format filled in.
        let mut config: Value = serde_json::from_str(
            &row.config
                .to_string()
                .replace("{bucket}", &bucket),
        )
        .expect("the row's configuration is JSON");
        if let Some(format) = row.compression.wire() {
            if row.member == "RedshiftDestinationConfiguration" {
                config["S3Configuration"]["CompressionFormat"] = json!(format);
            } else {
                config["CompressionFormat"] = json!(format);
            }
        }
        emulator
            .create_active_stream(&stream, row.member, config)
            .await;

        let records: Vec<Vec<u8>> = row
            .records
            .iter()
            .map(|record| record.as_bytes().to_vec())
            .collect();
        let (status, put) = emulator.put_batch(&stream, &records).await;
        assert_eq!(status, 200, "case {:?}: put got {put:?}", row.case);
        assert_eq!(
            put["FailedPutCount"],
            json!(0),
            "case {:?}: every record was accepted, got {put:?}",
            row.case
        );

        // Nothing is delivered before the buffer is due: a caller reading the destination
        // straight after its put still sees the data buffered.
        let immediate = emulator.keys(&bucket).await;
        assert!(
            immediate.is_empty(),
            "case {:?}: the buffer is delivered after its window, not during the put, \
             got {immediate:?}",
            row.case
        );

        let keys = emulator.delivered_keys(row.case, &bucket, 1).await;
        assert_eq!(
            keys.len(),
            1,
            "case {:?}: one buffer is one object, got {keys:?}",
            row.case
        );
        let key = keys[0].clone();
        assert!(
            key.starts_with(row.key_prefix),
            "case {:?}: expected a key under {:?}, got {key:?}",
            row.case,
            row.key_prefix
        );
        assert_eq!(
            carries_time_prefix(&key, row.key_prefix),
            row.automatic_time_prefix,
            "case {:?}: expected automatic time prefix {} in {key:?}",
            row.case,
            row.automatic_time_prefix
        );
        let extension = row
            .file_extension
            .unwrap_or_else(|| row.compression.extension());
        assert!(
            key.ends_with(extension),
            "case {:?}: expected a key ending {extension:?}, got {key:?}",
            row.case
        );
        assert!(
            key.contains(&stream),
            "case {:?}: a delivered object is named for its stream, got {key:?}",
            row.case
        );

        let expected: Vec<u8> = row
            .records
            .iter()
            .flat_map(|record| {
                record
                    .as_bytes()
                    .iter()
                    .copied()
                    .chain(row.separator.as_bytes().iter().copied())
            })
            .collect();
        let stored = emulator.object(&bucket, &key).await;
        let decoded = row.compression.decode(&stored);
        assert_eq!(
            String::from_utf8_lossy(&decoded),
            String::from_utf8_lossy(&expected),
            "case {:?}: the object holds the buffered records in arrival order",
            row.case
        );
        if row.compression != Compression::Uncompressed {
            assert_ne!(
                stored, expected,
                "case {:?}: the stored bytes are encoded, not the plain records",
                row.case
            );
        }

        // The buffer is delivered ONCE: the records left it when they were delivered.
        tokio::time::sleep(POLL_INTERVAL * 20).await;
        let settled = emulator.keys(&bucket).await;
        assert_eq!(
            settled, keys,
            "case {:?}: a delivered buffer is not delivered again",
            row.case
        );
    }
}

/// One dynamic-partitioning case: the jq program, the prefix it fills in, and the records
/// with the prefix each is expected to land under.
struct PartitionRow {
    case: &'static str,
    /// The `MetadataExtractionQuery` — a jq program, as `JsonParsingEngine` `JQ-1.6` names.
    query: &'static str,
    /// The destination `Prefix`, carrying the `partitionKeyFromQuery` namespace.
    prefix: &'static str,
    /// `(record, the prefix its object must be written under)`. A record expected in the
    /// error output carries the error prefix here.
    records: Vec<(&'static str, &'static str)>,
}

/// The dynamic-partitioning cases.
///
/// The rows differ along what the rule generalises over: how many partition keys the
/// program derives and where in the prefix they go, the TYPE of the derived value (a
/// string, a number, a boolean), a program that reads a nested member, the records a
/// program derives no key for at all — unreadable JSON, a member that is not there, a
/// value that is not a scalar — and a prefix naming a key only a Lambda data
/// transformation could supply: the last two are the records the error output prefix is
/// for.
fn partition_rows() -> Vec<PartitionRow> {
    vec![
        PartitionRow {
            case: "one string partition key",
            query: "{dept:.dept}",
            prefix: "p/!{partitionKeyFromQuery:dept}/",
            records: vec![
                (r#"{"dept":"sales","id":1}"#, "p/sales/"),
                (r#"{"dept":"sales","id":2}"#, "p/sales/"),
                (r#"{"dept":"engineering","id":3}"#, "p/engineering/"),
            ],
        },
        PartitionRow {
            case: "two partition keys, both named in the prefix",
            query: "{dept:.dept,region:.region}",
            prefix: "p/!{partitionKeyFromQuery:dept}/!{partitionKeyFromQuery:region}/",
            records: vec![
                (r#"{"dept":"ops","region":"eu"}"#, "p/ops/eu/"),
                (r#"{"dept":"ops","region":"us"}"#, "p/ops/us/"),
            ],
        },
        PartitionRow {
            case: "a numeric and a boolean partition value",
            query: "{year:.year,active:.active}",
            prefix: "y=!{partitionKeyFromQuery:year}/a=!{partitionKeyFromQuery:active}/",
            records: vec![
                (r#"{"year":2026,"active":true}"#, "y=2026/a=true/"),
                (r#"{"year":2025,"active":false}"#, "y=2025/a=false/"),
            ],
        },
        PartitionRow {
            case: "a program reading a nested member",
            query: "{customer:.customer.id}",
            prefix: "c/!{partitionKeyFromQuery:customer}/",
            records: vec![
                (r#"{"customer":{"id":"c-1"}}"#, "c/c-1/"),
                (r#"{"customer":{"id":"c-2"}}"#, "c/c-2/"),
            ],
        },
        PartitionRow {
            case: "records the program derives no key for go to the error output",
            query: "{dept:.dept}",
            prefix: "p/!{partitionKeyFromQuery:dept}/",
            records: vec![
                (r#"{"dept":"sales"}"#, "p/sales/"),
                // Not JSON at all, so the extraction cannot run on it.
                ("not json", EXTRACTION_FAILED_PREFIX),
                // JSON, but the program derives no value for the key the prefix names.
                (r#"{"other":"value"}"#, EXTRACTION_FAILED_PREFIX),
                // A derived value that is not a scalar is not one a prefix can carry.
                (r#"{"dept":{"nested":true}}"#, EXTRACTION_FAILED_PREFIX),
            ],
        },
        PartitionRow {
            case: "a prefix naming a key only a data transformation could supply",
            query: "{dept:.dept}",
            // `partitionKeyFromLambda` asks for the keys a Lambda data TRANSFORMATION
            // returned, and this emulator runs none — so the partition keys of these
            // records cannot be determined either.
            prefix: "p/!{partitionKeyFromLambda:dept}/",
            records: vec![
                (r#"{"dept":"sales"}"#, EXTRACTION_FAILED_PREFIX),
                (r#"{"dept":"engineering"}"#, EXTRACTION_FAILED_PREFIX),
            ],
        },
    ]
}

/// The `ErrorOutputPrefix` every partition case configures, before expansion: "A prefix
/// that Firehose evaluates and adds to failed records before writing them to S3"
/// (`ErrorOutputPrefix`).
const ERROR_OUTPUT_PREFIX: &str = "bad/!{firehose:error-output-type}/";

/// That prefix with the error output type of a record whose partition keys could not be
/// determined expanded.
const EXTRACTION_FAILED_PREFIX: &str = "bad/metadata-extraction-failed/";

/// The `errorCode` the error-output document of such a record names.
const EXTRACTION_FAILED_CODE: &str = "DynamicPartitioning.MetadataExtractionFailed";

/// The error documents an error-output object holds, one per line.
fn error_documents(body: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(body)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("an error-output document must be JSON: {error} in {line:?}")
            })
        })
        .collect()
}

/// The record an error document carries, decoded from its `rawData`.
fn raw_data(document: &Value) -> String {
    let encoded = document["rawData"]
        .as_str()
        .unwrap_or_else(|| panic!("an error-output document carries rawData: {document:?}"));
    let decoded = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("rawData must be base64: {error} in {document:?}"));
    String::from_utf8_lossy(&decoded).into_owned()
}

#[tokio::test]
async fn dynamic_partitioning_writes_each_partition_under_the_prefix_its_keys_derive() {
    for (ordinal, row) in partition_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("partition-bucket-{ordinal}");
        let stream = format!("partition-stream-{ordinal}");
        emulator.create_bucket(&bucket).await;

        let mut config = s3_destination(&bucket, Some(row.prefix));
        config["ErrorOutputPrefix"] = json!(ERROR_OUTPUT_PREFIX);
        config["DynamicPartitioningConfiguration"] = json!({ "Enabled": true });
        config["ProcessingConfiguration"] = json!({
            "Enabled": true,
            "Processors": [{
                "Type": "MetadataExtraction",
                "Parameters": [
                    { "ParameterName": "MetadataExtractionQuery", "ParameterValue": row.query },
                    { "ParameterName": "JsonParsingEngine", "ParameterValue": "JQ-1.6" },
                ],
            }],
        });
        emulator
            .create_active_stream(&stream, "ExtendedS3DestinationConfiguration", config)
            .await;

        let records: Vec<Vec<u8>> = row
            .records
            .iter()
            .map(|(record, _)| record.as_bytes().to_vec())
            .collect();
        let (status, put) = emulator.put_batch(&stream, &records).await;
        assert_eq!(status, 200, "case {:?}: put got {put:?}", row.case);

        // One object per distinct expected prefix.
        let mut wanted: Vec<&str> = row.records.iter().map(|(_, prefix)| *prefix).collect();
        wanted.sort_unstable();
        wanted.dedup();
        emulator
            .delivered_keys(row.case, &bucket, wanted.len())
            .await;
        let bodies = emulator.delivered_bodies(&bucket).await;
        assert_eq!(
            bodies.len(),
            wanted.len(),
            "case {:?}: expected one object per partition {wanted:?}, got {:?}",
            row.case,
            bodies.keys().collect::<Vec<_>>()
        );

        for (record, prefix) in &row.records {
            // A delivered object holds its records' own bytes; an ERROR OUTPUT object
            // holds one error document per record, which carries the record's bytes as
            // `rawData` and names what failed — so each is looked for the way it is
            // written.
            let holds = |key: &str, body: &[u8]| -> bool {
                if key.starts_with(EXTRACTION_FAILED_PREFIX) {
                    error_documents(body)
                        .iter()
                        .any(|document| raw_data(document) == *record)
                } else {
                    String::from_utf8_lossy(body).contains(record)
                }
            };
            let holder = bodies
                .iter()
                .find(|(key, body)| key.starts_with(prefix) && holds(key, body))
                .map(|(key, _)| key.clone());
            assert!(
                holder.is_some(),
                "case {:?}: expected {record:?} in an object under {prefix:?}, got {:?}",
                row.case,
                bodies
                    .iter()
                    .map(|(key, body)| (key.clone(), String::from_utf8_lossy(body).into_owned()))
                    .collect::<Vec<_>>()
            );
            // And nowhere else: a record belongs to exactly one partition.
            for (key, body) in &bodies {
                if Some(key) == holder.as_ref() {
                    continue;
                }
                assert!(
                    !holds(key, body),
                    "case {:?}: {record:?} is in {key:?} as well as its own partition",
                    row.case
                );
            }
            // Every document of the error output names why the record is there.
            if *prefix == EXTRACTION_FAILED_PREFIX {
                let key = holder.clone().unwrap_or_default();
                let documents = error_documents(&bodies[&key]);
                for document in &documents {
                    assert_eq!(
                        document["errorCode"],
                        json!(EXTRACTION_FAILED_CODE),
                        "case {:?}: the error document of {record:?} names the failure, \
                         got {document:?}",
                        row.case
                    );
                    assert!(
                        document["attemptsMade"].as_u64().is_some_and(|made| made >= 1)
                            && document["arrivalTimestamp"].as_u64().is_some()
                            && document["attemptEndingTimestamp"].as_u64().is_some(),
                        "case {:?}: the error document carries the attempt it reports, \
                         got {document:?}",
                        row.case
                    );
                }
            }
        }
    }
}

/// A `MetadataExtractionQuery` no sane configuration would carry: a program whose run
/// would cost unbounded work, or one reaching for something outside the record it is given.
/// The rule they probe is the confinement: every one of them FAILS CLOSED — its records go
/// to the error output, carrying the metadata-extraction failure — and the emulator keeps
/// serving, with another stream still delivering.
const HOSTILE_QUERIES: &[(&str, &str)] = &[
    ("a definition, which may call itself", "def f: f; f"),
    (
        "an unbounded generator",
        "{dept:(while(true; .)|tostring)}",
    ),
    ("an unbounded range", "{dept:([range(1000000000)]|length)}"),
    // The operators that GROW a value: repeated application of either doubles it.
    (
        "a value grown by repeated concatenation",
        "{dept:([.dept]|.+.|.+.|.+.|tostring)}",
    ),
    (
        "a string repeated a billion times",
        "{dept:(.dept*1000000000)}",
    ),
    // The other two operators that grow a value out of proportion to the record: in jq `/`
    // over two strings SPLITS one into an array of its pieces, and `-` over two arrays
    // subtracts one from the other by comparing every pair of their elements.
    (
        "a record split into an array of its pieces",
        "{dept:((.dept/\"\")|length|tostring)}",
    ),
    (
        "an array subtracted from another elementwise",
        "{dept:(([.dept]-[.dept])|length|tostring)}",
    ),
    // Assignment embeds a value into a field of itself, and it chains, so a handful of
    // tokens multiplies the record many times over.
    (
        "a value multiplied by a chain of assignments",
        "{dept:.dept}|.a=.|.b=.|.c=.",
    ),
    // A chain of constructions, which is how a program grows a value without an operator
    // that grows one: each stage holding two copies of its input doubles it.
    (
        "a value doubled by a chain of constructions",
        "{a:.,b:.}|{a:.,b:.}|{a:.,b:.}|{dept:\"x\"}",
    ),
    (
        "a program that interpolates",
        "{dept:\"\\(.dept)-\\(.dept)\"}",
    ),
    // The core looping forms: each re-applies its update expression once per element of
    // the value it is given, so one construction inside either is worth a chain of them.
    (
        "a reduction over the record",
        "{dept:(reduce .items[] as $i ([]; $i))|tostring}",
    ),
    (
        "a loop over the record",
        "{dept:(foreach .items[] as $i (0; .+0; $i))|tostring}",
    ),
    // The GENERATORS: an iteration hands the engine one output per element of its value and
    // a pipe re-applies what follows once per output, so two of them multiply, and jq's
    // recursive descent generates one output per sub-value at every depth of the record.
    (
        "two iterations multiplied by a pipe",
        "[.a[] | .b[]]",
    ),
    (
        "a recursive descent through the record",
        "[.. ]",
    ),
    // jq's two-value generator, chained: each link doubles what the next one is applied to,
    // so a handful of tokens makes the engine materialise many copies of the record.
    (
        "a value doubled by a chain of comma generators",
        "[(.,.)|(.,.)|(.,.)|(.,.)]",
    ),
    ("a program that does not parse", "{dept:"),
    (
        "a program naming a function that does not exist",
        "{dept:nosuchfunction}",
    ),
    // A definition hidden behind a jq comment, and behind a string literal that never
    // closes: both are read the way the engine reads them, so neither smuggles anything.
    (
        "a definition behind a comment",
        "# a comment\ndef f: f; f",
    ),
    (
        "a definition behind a string that never closes",
        "# \"\ndef f: f; f",
    ),
    (
        "a program carrying a string that never closes",
        "{dept:\"sales}",
    ),
    // The host escapes jq itself has: the process environment, a further input, and a
    // module the engine would have to read from disk. None of them is in this engine.
    ("the process environment, as a function", "{dept:env}"),
    ("the process environment, as a variable", "{dept:$ENV.PATH}"),
    ("another input", "{dept:input}"),
    ("the name of an input file", "{dept:input_filename}"),
    ("a module to import", "include \"other\"; {dept:.dept}"),
    (
        "a program nested deeper than this emulator reads",
        "{dept:((((((((((((((((((((((((((((((((((.dept))))))))))))))))))))))))))))))))))}",
    ),
];

#[tokio::test]
async fn a_hostile_metadata_extraction_program_fails_closed_and_leaves_the_emulator_serving() {
    let emulator = Emulator::new();
    let healthy_bucket = "hostile-healthy-bucket";
    let healthy_stream = "hostile-healthy-stream";
    emulator.create_bucket(healthy_bucket).await;
    emulator
        .create_active_stream(
            healthy_stream,
            "ExtendedS3DestinationConfiguration",
            s3_destination(healthy_bucket, Some("ok/")),
        )
        .await;

    for (ordinal, (case, query)) in HOSTILE_QUERIES.iter().enumerate() {
        let bucket = format!("hostile-bucket-{ordinal}");
        let stream = format!("hostile-stream-{ordinal}");
        emulator.create_bucket(&bucket).await;
        let mut config = s3_destination(&bucket, Some("h/!{partitionKeyFromQuery:dept}/"));
        config["ErrorOutputPrefix"] = json!(ERROR_OUTPUT_PREFIX);
        config["DynamicPartitioningConfiguration"] = json!({ "Enabled": true });
        config["ProcessingConfiguration"] = json!({
            "Enabled": true,
            "Processors": [{
                "Type": "MetadataExtraction",
                "Parameters": [
                    { "ParameterName": "MetadataExtractionQuery", "ParameterValue": query },
                    { "ParameterName": "JsonParsingEngine", "ParameterValue": "JQ-1.6" },
                ],
            }],
        });
        emulator
            .create_active_stream(&stream, "ExtendedS3DestinationConfiguration", config)
            .await;
        let record = br#"{"dept":"sales"}"#.to_vec();
        let (status, put) = emulator
            .put_batch(&stream, std::slice::from_ref(&record))
            .await;
        assert_eq!(status, 200, "case {case:?}: the put is accepted, got {put:?}");

        // The record is delivered to the ERROR OUTPUT, never under a derived prefix and
        // never dropped: the program produced no partition keys, whatever it tried.
        let keys = emulator.delivered_keys(case, &bucket, 1).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: one error-output object, got {keys:?}"
        );
        assert!(
            keys[0].starts_with(EXTRACTION_FAILED_PREFIX),
            "case {case:?}: expected the record under {EXTRACTION_FAILED_PREFIX:?}, \
             got {:?}",
            keys[0]
        );
        let documents = error_documents(&emulator.object(&bucket, &keys[0]).await);
        assert_eq!(
            documents.len(),
            1,
            "case {case:?}: one error document, got {documents:?}"
        );
        assert_eq!(
            documents[0]["errorCode"],
            json!(EXTRACTION_FAILED_CODE),
            "case {case:?}: got {:?}",
            documents[0]
        );
        assert_eq!(
            raw_data(&documents[0]),
            String::from_utf8_lossy(&record),
            "case {case:?}: the error document carries the record itself"
        );
    }

    // The emulator is still serving: a stream whose program is ordinary still delivers,
    // and every call still answers.
    let (status, put) = emulator
        .put_batch(healthy_stream, &[b"still-delivering".to_vec()])
        .await;
    assert_eq!(status, 200, "the healthy stream's put got {put:?}");
    let keys = emulator
        .delivered_keys("a healthy stream beside the hostile ones", healthy_bucket, 1)
        .await;
    assert_eq!(
        keys.len(),
        1,
        "a hostile program on one stream must not stop another delivering, got {keys:?}"
    );
}

/// A program that CARRIES the word `def` without defining anything — in a string literal
/// and inside a longer name — is read as the program it is, and its keys are derived.
#[tokio::test]
async fn a_program_mentioning_a_definition_without_making_one_still_derives_its_keys() {
    let emulator = Emulator::new();
    let bucket = "definition-lookalike-bucket";
    let stream = "definition-lookalike-stream";
    emulator.create_bucket(bucket).await;
    let mut config = s3_destination(bucket, Some("d/!{partitionKeyFromQuery:default_dept}/"));
    config["ErrorOutputPrefix"] = json!(ERROR_OUTPUT_PREFIX);
    config["DynamicPartitioningConfiguration"] = json!({ "Enabled": true });
    config["ProcessingConfiguration"] = json!({
        "Enabled": true,
        "Processors": [{
            "Type": "MetadataExtraction",
            // `default_dept` carries `def` inside a longer name, and the string literal
            // carries it on its own: neither defines a function.
            "Parameters": [
                {
                    "ParameterName": "MetadataExtractionQuery",
                    "ParameterValue": "{default_dept:(.dept // \"def\")}",
                },
                { "ParameterName": "JsonParsingEngine", "ParameterValue": "JQ-1.6" },
            ],
        }],
    });
    emulator
        .create_active_stream(stream, "ExtendedS3DestinationConfiguration", config)
        .await;

    let (status, put) = emulator
        .put_batch(stream, &[br#"{"dept":"sales"}"#.to_vec()])
        .await;
    assert_eq!(status, 200, "the put got {put:?}");
    let keys = emulator
        .delivered_keys("a program mentioning a definition", bucket, 1)
        .await;
    assert!(
        keys[0].starts_with("d/sales/"),
        "expected the derived partition prefix, got {:?}",
        keys[0]
    );
}

/// A record larger than the most one metadata-extraction run is given has no partition
/// keys derived and goes to the error output, while a record inside that bound, in the same
/// buffer, lands under its derived prefix: what decides it is the record's SIZE and nothing
/// else about it.
#[tokio::test]
async fn a_record_too_large_to_extract_from_goes_to_the_error_output() {
    let emulator = Emulator::new();
    let bucket = "oversized-record-bucket";
    let stream = "oversized-record-stream";
    emulator.create_bucket(bucket).await;
    let mut config = s3_destination(bucket, Some("p/!{partitionKeyFromQuery:dept}/"));
    config["ErrorOutputPrefix"] = json!(ERROR_OUTPUT_PREFIX);
    config["DynamicPartitioningConfiguration"] = json!({ "Enabled": true });
    config["ProcessingConfiguration"] = json!({
        "Enabled": true,
        "Processors": [{
            "Type": "MetadataExtraction",
            "Parameters": [
                { "ParameterName": "MetadataExtractionQuery", "ParameterValue": "{dept:.dept}" },
                { "ParameterName": "JsonParsingEngine", "ParameterValue": "JQ-1.6" },
            ],
        }],
    });
    emulator
        .create_active_stream(stream, "ExtendedS3DestinationConfiguration", config)
        .await;

    // Both records carry the partition key the query reads; only their size differs.
    let small = br#"{"dept":"sales","padding":"x"}"#.to_vec();
    let large = format!(
        r#"{{"dept":"sales","padding":"{}"}}"#,
        "x".repeat(256 * 1024)
    )
    .into_bytes();
    let (status, put) = emulator.put_batch(stream, &[small, large.clone()]).await;
    assert_eq!(status, 200, "the put got {put:?}");

    let keys = emulator
        .delivered_keys("a record too large to extract from", bucket, 2)
        .await;
    let derived: Vec<&String> = keys.iter().filter(|key| key.starts_with("p/sales/")).collect();
    let errored: Vec<&String> = keys
        .iter()
        .filter(|key| key.starts_with(EXTRACTION_FAILED_PREFIX))
        .collect();
    assert_eq!(
        (derived.len(), errored.len()),
        (1, 1),
        "expected one derived partition and one error output, got {keys:?}"
    );
    let documents = error_documents(&emulator.object(bucket, errored[0]).await);
    assert_eq!(
        documents.len(),
        1,
        "the oversized record is the only one in the error output, got {documents:?}"
    );
    assert_eq!(
        documents[0]["errorCode"],
        json!(EXTRACTION_FAILED_CODE),
        "got {:?}",
        documents[0]
    );
    assert_eq!(
        raw_data(&documents[0]).len(),
        large.len(),
        "the error document carries the oversized record itself"
    );
}

#[tokio::test]
async fn an_update_destination_repointing_the_bucket_moves_every_later_delivery() {
    let emulator = Emulator::new();
    let stream = "repointed-stream";
    let first = "repoint-bucket-one";
    let second = "repoint-bucket-two";
    emulator.create_bucket(first).await;
    emulator.create_bucket(second).await;
    emulator
        .create_active_stream(
            stream,
            "ExtendedS3DestinationConfiguration",
            s3_destination(first, Some("r/")),
        )
        .await;

    // Before the switch: the records land in the bucket the stream was created with.
    let (status, put) = emulator
        .put_batch(stream, &[b"before-the-switch".to_vec()])
        .await;
    assert_eq!(status, 200, "the first put got {put:?}");
    let before = emulator
        .delivered_keys("before the switch", first, 1)
        .await;
    assert_eq!(before.len(), 1, "before the switch: got {before:?}");

    // Repoint the destination at the second bucket.
    let described = emulator
        .firehose_ok(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    let description = &described["DeliveryStreamDescription"];
    let version = description["VersionId"]
        .as_str()
        .expect("the description carries a version id")
        .to_string();
    let destination_id = description["Destinations"][0]["DestinationId"]
        .as_str()
        .expect("the description carries a destination id")
        .to_string();
    emulator
        .firehose_ok(
            "UpdateDestination",
            json!({
                "DeliveryStreamName": stream,
                "CurrentDeliveryStreamVersionId": version,
                "DestinationId": destination_id,
                "ExtendedS3DestinationUpdate": {
                    "BucketARN": format!("arn:aws:s3:::{second}"),
                },
            }),
        )
        .await;

    // After the switch: the later records land in the new bucket, and the old one stops
    // receiving.
    let (status, put) = emulator
        .put_batch(stream, &[b"after-the-switch".to_vec()])
        .await;
    assert_eq!(status, 200, "the second put got {put:?}");
    let after = emulator.delivered_keys("after the switch", second, 1).await;
    assert_eq!(after.len(), 1, "after the switch: got {after:?}");

    let old_bodies = emulator.delivered_bodies(first).await;
    assert_eq!(
        old_bodies.len(),
        1,
        "the repointed-away bucket takes nothing further, got {:?}",
        old_bodies.keys().collect::<Vec<_>>()
    );
    for (key, body) in &old_bodies {
        let text = String::from_utf8_lossy(body).into_owned();
        assert!(
            text.contains("before-the-switch") && !text.contains("after-the-switch"),
            "the old bucket holds only what was put before the switch, {key:?} holds {text:?}"
        );
    }
    let new_bodies = emulator.delivered_bodies(second).await;
    for (key, body) in &new_bodies {
        let text = String::from_utf8_lossy(body).into_owned();
        assert!(
            text.contains("after-the-switch") && !text.contains("before-the-switch"),
            "the new bucket holds only what was put after the switch, {key:?} holds {text:?}"
        );
    }
    // The update also keeps the prefix it did not name.
    for key in new_bodies.keys() {
        assert!(
            key.starts_with("r/"),
            "the update changed only the bucket, so the prefix still applies: {key:?}"
        );
    }
}

/// One refusal case: a put the boundary refuses, and a sibling put of the same shape that
/// is one unit inside the ceiling.
struct CeilingRow {
    case: &'static str,
    /// The records of the refused put.
    refused: Vec<Vec<u8>>,
    /// The marker every refused record carries, which must reach no object.
    refused_marker: &'static str,
    /// The records of the accepted put, in the same buffer window.
    accepted: Vec<Vec<u8>>,
    /// The marker every accepted record carries, which must reach an object.
    accepted_marker: &'static str,
}

/// The ceiling cases: the modelled bounds of `PutRecordBatch`
/// (`PutRecordBatchRequestEntryList`, len 1..500) and of one record
/// (`Record$Data`/`Data`, len 0..1,024,000), each probed at the published bound and one
/// past it.
fn ceiling_rows() -> Vec<CeilingRow> {
    let marker = |tag: &str, ordinal: usize| format!("[{tag}-{ordinal}]").into_bytes();
    vec![
        CeilingRow {
            case: "a batch one record past the 500-record ceiling",
            refused: (0..501).map(|index| marker("over", index)).collect(),
            refused_marker: "[over-",
            accepted: (0..500).map(|index| marker("at", index)).collect(),
            accepted_marker: "[at-",
        },
        CeilingRow {
            case: "a record one byte past the 1,024,000-byte ceiling",
            refused: vec![sized_record("[big]", 1_024_001)],
            refused_marker: "[big]",
            accepted: vec![sized_record("[fits]", 1_024_000)],
            accepted_marker: "[fits]",
        },
    ]
}

/// A record of exactly `bytes` bytes carrying `marker` at its start.
fn sized_record(marker: &str, bytes: usize) -> Vec<u8> {
    let mut record = marker.as_bytes().to_vec();
    record.resize(bytes, b'.');
    record
}

#[tokio::test]
async fn a_refused_put_delivers_nothing_while_a_sibling_put_in_the_same_window_arrives() {
    for (ordinal, row) in ceiling_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("ceiling-bucket-{ordinal}");
        let stream = format!("ceiling-stream-{ordinal}");
        emulator.create_bucket(&bucket).await;
        emulator
            .create_active_stream(
                &stream,
                "ExtendedS3DestinationConfiguration",
                s3_destination(&bucket, Some("c/")),
            )
            .await;

        // The put past the ceiling is refused, and nothing of it is stored.
        let (status, refused) = emulator.put_batch(&stream, &row.refused).await;
        assert_eq!(
            status, 400,
            "case {:?}: the put past the ceiling is refused, got {status} {refused:?}",
            row.case
        );

        // A sibling put inside the ceiling, in the same buffer window, does arrive.
        let (status, accepted) = emulator.put_batch(&stream, &row.accepted).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put inside the ceiling is accepted, got {accepted:?}",
            row.case
        );
        assert_eq!(
            accepted["FailedPutCount"],
            json!(0),
            "case {:?}: every record inside the ceiling was accepted, got {accepted:?}",
            row.case
        );

        emulator.delivered_keys(row.case, &bucket, 1).await;
        let delivered: String = emulator
            .delivered_bodies(&bucket)
            .await
            .into_values()
            .map(|body| String::from_utf8_lossy(&body).into_owned())
            .collect();
        assert!(
            !delivered.contains(row.refused_marker),
            "case {:?}: no record of the refused put is delivered",
            row.case
        );
        let arrived = delivered.matches(row.accepted_marker).count();
        assert_eq!(
            arrived,
            row.accepted.len(),
            "case {:?}: every record of the accepted put is delivered",
            row.case
        );
    }
}

/// How the records of a [`StaticPrefixRow`] reach the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Put {
    /// One `PutRecord` call per record, which is how a producer that writes its records
    /// one at a time reaches the stream.
    OneCallEach,
    /// One `PutRecordBatch` carrying every record.
    OneBatch,
}

/// One delivery case for a stream whose destination names a STATIC prefix — one carrying
/// no `!{...}` expression of any kind.
struct StaticPrefixRow {
    case: &'static str,
    /// The `DeliveryStreamType` the create names, or `None` to leave the member out.
    stream_type: Option<&'static str>,
    /// The destination `Prefix`, with `{stream}` standing for the row's own stream name;
    /// `None` leaves the member out.
    prefix: Option<&'static str>,
    /// The destination `BufferingHints`, or `None` for the modelled defaults.
    hints: Option<Value>,
    /// Whether the destination configures [`ERROR_OUTPUT_PREFIX`], which — with no dynamic
    /// partitioning for a record to fail — must take nothing.
    error_output: bool,
    put: Put,
    /// The records put, in arrival order.
    records: Vec<&'static str>,
}

/// The static-prefix cases.
///
/// The rows differ along what the rule generalises over: whether the create names
/// `DeliveryStreamType` at all, the FORM of the static prefix (named for the stream, a
/// nested path, one not ending in the delimiter, and none at all), the `SizeInMBs` of the
/// `BufferingHints` (each published bound), how the
/// records are put (a call each or one batch), the record count, and whether an
/// `ErrorOutputPrefix` is configured.
fn static_prefix_rows() -> Vec<StaticPrefixRow> {
    // One JSON object per record, newline terminated, as a producer writing a queryable
    // raw-JSON prefix puts them.
    let lines = vec![
        "{\"id\":1,\"event\":\"created\"}\n",
        "{\"id\":2,\"event\":\"updated\"}\n",
        "{\"id\":3,\"event\":\"renamed\"}\n",
        "{\"id\":4,\"event\":\"deleted\"}\n",
        "{\"id\":5,\"event\":\"restored\"}\n",
    ];
    vec![
        StaticPrefixRow {
            case: "a stream written to one record at a time, under a prefix named for it",
            stream_type: Some("DirectPut"),
            prefix: Some("{stream}/"),
            hints: Some(json!({ "SizeInMBs": 1, "IntervalInSeconds": 0 })),
            error_output: true,
            put: Put::OneCallEach,
            records: lines.clone(),
        },
        StaticPrefixRow {
            case: "the same destination reached with one batch instead of a call per record",
            stream_type: Some("DirectPut"),
            prefix: Some("{stream}/"),
            hints: Some(json!({ "SizeInMBs": 1, "IntervalInSeconds": 0 })),
            error_output: true,
            put: Put::OneBatch,
            records: lines.clone(),
        },
        StaticPrefixRow {
            case: "a nested static prefix, the hints the helper configures, and no stream \
                   type",
            stream_type: None,
            prefix: Some("raw/events/json/"),
            hints: None,
            error_output: true,
            put: Put::OneCallEach,
            records: vec![lines[0]],
        },
        StaticPrefixRow {
            case: "a static prefix that does not end in the delimiter",
            stream_type: Some("DirectPut"),
            prefix: Some("{stream}-"),
            hints: Some(json!({ "SizeInMBs": 128, "IntervalInSeconds": 0 })),
            error_output: true,
            put: Put::OneCallEach,
            records: vec![lines[1], lines[2]],
        },
        StaticPrefixRow {
            case: "no prefix at all, so the automatic segments are the whole key",
            stream_type: Some("DirectPut"),
            prefix: None,
            hints: Some(json!({ "SizeInMBs": 1, "IntervalInSeconds": 0 })),
            error_output: false,
            put: Put::OneCallEach,
            records: vec![lines[3], lines[4]],
        },
    ]
}

/// The literal part of [`ERROR_OUTPUT_PREFIX`], before its first expression: the prefix a
/// client lists to see whether ANYTHING was diverted to the error output.
fn error_output_root() -> &'static str {
    ERROR_OUTPUT_PREFIX.split("!{").next().unwrap_or_default()
}

/// A stream whose destination names a static prefix delivers its buffered records under
/// that prefix followed by the service's own date segments, and diverts nothing to its
/// error output.
///
/// The rule: for a destination whose `Prefix` carries no expression, the key of a
/// delivered object is that prefix followed by the automatic `YYYY/MM/DD/HH/` segments
/// ("The \"YYYY/MM/DD/HH\" time format prefix is automatically used for delivered Amazon
/// S3 files" — `Prefix`) and then the object's own name, whichever put accepted the
/// records; and a destination with no dynamic partitioning writes nothing under its
/// `ErrorOutputPrefix`, which is for "failed records" only (`ErrorOutputPrefix`). So the
/// objects sit strictly BELOW the configured prefix and a client reading that prefix must
/// recurse to find them.
#[tokio::test]
async fn a_static_prefix_takes_the_automatic_date_segments_and_nothing_goes_to_the_error_output() {
    for (ordinal, row) in static_prefix_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("static-prefix-bucket-{ordinal}");
        let stream = format!("static-prefix-stream-{ordinal}");
        emulator.create_bucket(&bucket).await;

        let prefix = row
            .prefix
            .map(|prefix| prefix.replace("{stream}", &stream))
            .unwrap_or_default();
        let mut config = s3_destination(&bucket, row.prefix.map(|_| prefix.as_str()));
        if let Some(hints) = &row.hints {
            config["BufferingHints"] = hints.clone();
        }
        if row.error_output {
            config["ErrorOutputPrefix"] = json!(ERROR_OUTPUT_PREFIX);
        }
        emulator
            .create_active_stream_typed(
                &stream,
                row.stream_type,
                "ExtendedS3DestinationConfiguration",
                config,
            )
            .await;

        // The data plane is reachable only once the stream reports itself ACTIVE, and a
        // create naming no type made a stream a producer writes to directly.
        let described = emulator
            .firehose_ok(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": &stream }),
            )
            .await;
        let description = &described["DeliveryStreamDescription"];
        assert_eq!(
            description["DeliveryStreamStatus"],
            json!("ACTIVE"),
            "case {:?}: the stream must reach ACTIVE, got {description:?}",
            row.case
        );
        assert_eq!(
            description["DeliveryStreamType"],
            json!("DirectPut"),
            "case {:?}: a create naming {:?} makes a DirectPut stream, got {description:?}",
            row.case,
            row.stream_type
        );

        let records: Vec<Vec<u8>> = row
            .records
            .iter()
            .map(|record| record.as_bytes().to_vec())
            .collect();
        match row.put {
            Put::OneCallEach => {
                let mut ids = std::collections::BTreeSet::new();
                for record in &records {
                    let (status, answered) = emulator.put_record(&stream, record).await;
                    assert_eq!(
                        status, 200,
                        "case {:?}: PutRecord got {status} {answered:?}",
                        row.case
                    );
                    let id = answered["RecordId"].as_str().unwrap_or_default().to_string();
                    assert!(
                        !id.is_empty(),
                        "case {:?}: PutRecord answers a RecordId, got {answered:?}",
                        row.case
                    );
                    assert!(
                        ids.insert(id.clone()),
                        "case {:?}: each accepted record gets its own RecordId, got {id:?} twice",
                        row.case
                    );
                }
            }
            Put::OneBatch => {
                let (status, answered) = emulator.put_batch(&stream, &records).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: PutRecordBatch got {status} {answered:?}",
                    row.case
                );
                assert_eq!(
                    answered["FailedPutCount"],
                    json!(0),
                    "case {:?}: every record was accepted, got {answered:?}",
                    row.case
                );
            }
        }

        // Wait for the delivery the put promised: every byte the records hold is readable
        // out of the destination under the configured prefix.
        let expected: String = row.records.concat();
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        let delivered = loop {
            let mut bodies: BTreeMap<String, Vec<u8>> = BTreeMap::new();
            for key in emulator.keys_under(&bucket, &prefix).await {
                let body = emulator.object(&bucket, &key).await;
                bodies.insert(key, body);
            }
            let held: usize = bodies.values().map(Vec::len).sum();
            if held >= expected.len() {
                break bodies;
            }
            assert!(
                Instant::now() < deadline,
                "case {:?}: expected {} delivered byte(s) under {prefix:?}, got {:?}",
                row.case,
                expected.len(),
                bodies.keys().collect::<Vec<_>>()
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert!(
            !delivered.is_empty(),
            "case {:?}: the buffered records are delivered under {prefix:?}",
            row.case
        );

        for (key, body) in &delivered {
            assert!(
                carries_time_prefix(key, &prefix),
                "case {:?}: expected the automatic yyyy/MM/dd/HH segments after {prefix:?}, \
                 got {key:?}",
                row.case
            );
            let name = key[prefix.len()..].splitn(5, '/').nth(4).unwrap_or_default();
            assert!(
                !name.is_empty(),
                "case {:?}: an object name follows the date segments, got {key:?}",
                row.case
            );
            let text = String::from_utf8_lossy(body).into_owned();
            assert!(
                !text.is_empty() && expected.contains(&text),
                "case {:?}: {key:?} holds a non-empty run of the buffered records in arrival \
                 order, got {text:?}",
                row.case
            );
        }

        // Exactly the records put, each delivered once.
        let joined: String = delivered
            .values()
            .map(|body| String::from_utf8_lossy(body).into_owned())
            .collect();
        assert_eq!(
            joined.len(),
            expected.len(),
            "case {:?}: the delivered objects hold exactly the records put, got {joined:?}",
            row.case
        );
        for record in &row.records {
            assert_eq!(
                joined.matches(record).count(),
                1,
                "case {:?}: {record:?} is delivered exactly once, got {joined:?}",
                row.case
            );
        }

        // Nothing is diverted to the error output: there is no dynamic partitioning here
        // for a record to fail.
        if row.error_output {
            let errored = emulator.keys_under(&bucket, error_output_root()).await;
            assert!(
                errored.is_empty(),
                "case {:?}: a destination with no dynamic partitioning diverts nothing to \
                 its ErrorOutputPrefix, got {errored:?}",
                row.case
            );
        }

        // The objects sit BELOW the date segments, so reading the configured prefix has to
        // recurse: a `/`-delimited listing at that prefix reports no object of its own,
        // only the rolled-up date segment.
        let (at_level, entries) = emulator.delimited_listing(&bucket, &prefix).await;
        assert!(
            at_level.is_empty() && entries > 0,
            "case {:?}: expected the delivered objects below the date segments, a delimited \
             listing at {prefix:?} reported keys {at_level:?} and {entries} entry/entries",
            row.case
        );

        // A delivered buffer is not delivered again.
        tokio::time::sleep(POLL_INTERVAL * 20).await;
        let mut settled = emulator.keys_under(&bucket, &prefix).await;
        settled.sort();
        let expected_keys: Vec<String> = delivered.keys().cloned().collect();
        assert_eq!(
            settled, expected_keys,
            "case {:?}: a delivered buffer is not delivered again",
            row.case
        );
    }
}
