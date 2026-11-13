// SPDX-License-Identifier: CC0-1.0

//! The delivery-stream lifecycle end to end over the `awsJson1_1` wire: create ->
//! describe -> list -> put-record -> put-record-batch -> delete -> describe-gone, plus the
//! rules those operations apply.
//!
//! Every request goes through the real router (`POST /` with
//! `X-Amz-Target: Firehose_20150804.<Op>`), so what is asserted here is the wire
//! behaviour a client sees, not a handler called directly.
//!
//! The harness ([`Emulator`], [`s3_configuration`], [`error_code`]) is `pub(crate)` so a
//! sibling suite in this test target reuses it instead of standing up a second one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::{
    ComAmazonawsFirehoseFirehose20150804State, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test's requests are signed for, so the ARNs they mint name it.
///
/// `pub(crate)` because every suite in this test crate signs for the same Region.
pub(crate) const REGION: &str = "us-west-2";

/// Long enough to cover the emulator's chosen `CREATING` -> `ACTIVE` /
/// `DELETING` -> removed / `ENABLING` -> `ENABLED` span (`crate::state::TRANSITION_DELAY`),
/// with margin for a loaded machine.
pub(crate) const PAST_THE_TRANSITION: Duration = Duration::from_millis(3_400);

/// How long [`Emulator::await_reported`] keeps looking for the status it waits on before
/// it gives up. Generous on purpose: it bounds nothing the tests measure, it only stops a
/// wait that will never end, so a loaded or slow machine stays inside it.
pub(crate) const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// One emulator: the firehose service mounted on a single shared engine, plus the engine
/// itself so a test can read the record buffer no operation exposes yet.
///
/// The whole test crate's harness: a sibling suite reaches it as
/// `crate::delivery_stream_lifecycle::Emulator` rather than building a second one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// The single engine every operation of this emulator shares, for a test that has to
    /// read or seed state no modelled operation exposes.
    pub(crate) fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Call one operation; answer its HTTP status and JSON body.
    pub(crate) async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Firehose_20150804.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/firehose/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
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

    /// `(RecordId, Data)` of every record buffered for `stream`, in arrival order. No
    /// modelled operation reports the buffer, so the test reads the state the puts wrote.
    pub(crate) fn buffered(&self, stream: &str) -> Vec<(String, Vec<u8>)> {
        let state = self
            .engine
            .state::<ComAmazonawsFirehoseFirehose20150804State>();
        let store = state.lock();
        store
            .streams
            .get(stream)
            .map(|stream| {
                stream
                    .records
                    .iter()
                    .map(|record| (record.record_id.clone(), record.data.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Create a stream with the smallest valid S3 destination. It is `CREATING` on
    /// return; a test that needs it `ACTIVE` waits for that with [`Emulator::await_active`].
    pub(crate) async fn create_stream(&self, name: &str, stream_type: Option<&str>) {
        let mut body = json!({
            "DeliveryStreamName": name,
            "S3DestinationConfiguration": s3_configuration(),
        });
        match stream_type {
            Some("KinesisStreamAsSource") => {
                body["DeliveryStreamType"] = json!("KinesisStreamAsSource");
                body["KinesisStreamSourceConfiguration"] = json!({
                    "KinesisStreamARN": format!("arn:aws:kinesis:{REGION}:000000000000:stream/{name}"),
                    "RoleARN": ROLE_ARN,
                });
            }
            Some("MSKAsSource") => {
                body["DeliveryStreamType"] = json!("MSKAsSource");
                body["MSKSourceConfiguration"] = json!({
                    "MSKClusterARN": format!("arn:aws:kafka:{REGION}:000000000000:cluster/{name}/1"),
                    "TopicName": "events",
                    "AuthenticationConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "Connectivity": "PRIVATE",
                    },
                });
            }
            Some(other) => body["DeliveryStreamType"] = json!(other),
            None => {}
        }
        let (status, response) = self.call("CreateDeliveryStream", body).await;
        assert_eq!(status, 200, "create {name}: got {response:?}");
    }

    /// Wait for `name` to report the `ACTIVE` status the clock owes a freshly created
    /// stream, which is when an operation that needs a settled stream is served.
    pub(crate) async fn await_active(&self, name: &str) {
        self.await_reported(name, "DeliveryStreamDescription/DeliveryStreamStatus", "ACTIVE")
            .await;
    }

    /// Wait for what `DescribeDeliveryStream` reports for `name` at `path` to BE
    /// `expected`, by looking again until it is.
    ///
    /// The wait is on the condition and not on a duration: nothing here depends on how
    /// fast the host runs, and [`SETTLE_DEADLINE`] only stops a wait that would never
    /// end.
    pub(crate) async fn await_reported(&self, name: &str, path: &str, expected: &str) {
        let deadline = Instant::now() + SETTLE_DEADLINE;
        loop {
            let (status, described) = self
                .call(
                    "DescribeDeliveryStream",
                    json!({ "DeliveryStreamName": name }),
                )
                .await;
            let reported = if status == 200 {
                at(&described, path).as_str().unwrap_or_default().to_string()
            } else {
                format!("HTTP {status} {described:?}")
            };
            if reported == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{name}: expected {path} to reach {expected:?}, it is {reported:?}"
            );
            // Look again rather than wait out a span: the next read is what settles it.
            tokio::task::yield_now().await;
        }
    }
}

/// An IAM role ARN satisfying `RoleARN`'s modelled pattern.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-delivery";
/// An S3 bucket ARN satisfying `BucketARN`'s modelled pattern.
pub(crate) const BUCKET_ARN: &str = "arn:aws:s3:::demo-bucket";

pub(crate) fn s3_configuration() -> Value {
    json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN, "Prefix": "raw/" })
}

/// The wire error code of an error response: `__type` without its namespace prefix.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
        .unwrap_or_else(|| format!("no __type in {body:?}"))
}

/// The `message` of an error body. The firehose exceptions model the member as
/// `message`; a refusal the framework renders uses the same key.
///
/// `pub(crate)` beside [`error_code`] so every suite in this test crate reads a refusal
/// the same way instead of carrying its own copy.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("no message in {body:?}"))
}

/// Walk a `/`-separated path of object keys.
pub(crate) fn at<'v>(value: &'v Value, path: &str) -> &'v Value {
    let mut cursor = value;
    for key in path.split('/') {
        cursor = &cursor[key];
    }
    cursor
}

#[tokio::test]
async fn a_stream_is_created_described_listed_written_to_and_deleted() {
    let emulator = Emulator::new();

    // CREATE answers the ARN in the modelled form, naming the signing Region.
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "demo-stream",
                "DeliveryStreamType": "DirectPut",
                "S3DestinationConfiguration": s3_configuration(),
                "Tags": [{ "Key": "team", "Value": "data" }],
            }),
        )
        .await;
    assert_eq!(status, 200, "create: got {created:?}");
    assert_eq!(
        created["DeliveryStreamARN"],
        json!(format!(
            "arn:aws:firehose:{REGION}:000000000000:deliverystream/demo-stream"
        )),
        "create: the ARN names the Region, the emulator account and the stream"
    );

    // DESCRIBE, called first so no other work can elapse the creation span, shows the
    // transient status the real service shows: creating a stream is asynchronous.
    let (status, describing) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "demo-stream" }),
        )
        .await;
    assert_eq!(status, 200, "describe while creating: got {describing:?}");
    let description = &describing["DeliveryStreamDescription"];
    assert_eq!(
        description["DeliveryStreamStatus"],
        json!("CREATING"),
        "describe while creating: got {description:?}"
    );
    assert_eq!(
        description["DeliveryStreamName"],
        json!("demo-stream"),
        "describe while creating: got {description:?}"
    );
    assert_eq!(
        description["DeliveryStreamARN"], created["DeliveryStreamARN"],
        "describe while creating: the described ARN is the created one"
    );
    assert_eq!(
        description["DeliveryStreamType"],
        json!("DirectPut"),
        "describe while creating: got {description:?}"
    );
    assert_eq!(
        description["VersionId"],
        json!("1"),
        "describe while creating: a new stream is at version 1, got {description:?}"
    );
    assert!(
        description["CreateTimestamp"].is_number(),
        "describe while creating: CreateTimestamp is set, got {description:?}"
    );
    assert_eq!(
        description["HasMoreDestinations"],
        json!(false),
        "describe while creating: got {description:?}"
    );
    let destination = &description["Destinations"][0];
    assert_eq!(
        destination["DestinationId"],
        json!("destinationId-000000000001"),
        "describe while creating: got {destination:?}"
    );
    assert_eq!(
        at(destination, "S3DestinationDescription/BucketARN"),
        &json!(BUCKET_ARN),
        "describe while creating: the destination carries the config supplied at create, \
         got {destination:?}"
    );

    // A put before the stream is ACTIVE is refused: "Attempts to send data to a delivery
    // stream that is not in the ACTIVE state cause an exception."
    let (status, too_early) = emulator
        .call(
            "PutRecord",
            json!({
                "DeliveryStreamName": "demo-stream",
                "Record": { "Data": aws_smithy_types::base64::encode("hello") },
            }),
        )
        .await;
    assert_eq!(status, 400, "put while creating: got {too_early:?}");
    assert_eq!(
        error_code(&too_early),
        "ResourceNotFoundException",
        "put while creating: got {too_early:?}"
    );

    // A second create for the same name is refused whatever state the stream is in.
    let (status, conflict) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "demo-stream",
                "S3DestinationConfiguration": s3_configuration(),
            }),
        )
        .await;
    assert_eq!(status, 400, "duplicate create: got {conflict:?}");
    assert_eq!(
        error_code(&conflict),
        "ResourceInUseException",
        "duplicate create: got {conflict:?}"
    );

    // The clock owes the stream ACTIVE; nothing else does.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (_, active) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "demo-stream" }),
        )
        .await;
    assert_eq!(
        active["DeliveryStreamDescription"]["DeliveryStreamStatus"],
        json!("ACTIVE"),
        "describe after the creation span: got {active:?}"
    );

    // LIST reports the stored name.
    let (status, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(status, 200, "list: got {listed:?}");
    assert_eq!(
        listed["DeliveryStreamNames"],
        json!(["demo-stream"]),
        "list: got {listed:?}"
    );
    assert_eq!(
        listed["HasMoreDeliveryStreams"],
        json!(false),
        "list: got {listed:?}"
    );

    // PUT RECORD answers an id, and the record is in the buffer under that id.
    let (status, put) = emulator
        .call(
            "PutRecord",
            json!({
                "DeliveryStreamName": "demo-stream",
                "Record": { "Data": aws_smithy_types::base64::encode("hello") },
            }),
        )
        .await;
    assert_eq!(status, 200, "put record: got {put:?}");
    let single_id = put["RecordId"].as_str().unwrap_or_default().to_string();
    assert!(
        !single_id.is_empty(),
        "put record: RecordId is non-empty, got {put:?}"
    );
    assert_eq!(
        put["Encrypted"],
        json!(false),
        "put record: the stream has no SSE, got {put:?}"
    );

    // PUT RECORD BATCH answers one entry per input record, positionally aligned, each
    // with its own distinct id.
    let batch: Vec<&str> = vec!["one", "two", "three"];
    let (status, batched) = emulator
        .call(
            "PutRecordBatch",
            json!({
                "DeliveryStreamName": "demo-stream",
                "Records": batch
                    .iter()
                    .map(|data| json!({ "Data": aws_smithy_types::base64::encode(data) }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
    assert_eq!(status, 200, "put record batch: got {batched:?}");
    assert_eq!(
        batched["FailedPutCount"],
        json!(0),
        "put record batch: got {batched:?}"
    );
    let entries = batched["RequestResponses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        batch.len(),
        "put record batch: one entry per input record, got {batched:?}"
    );
    let mut ids: Vec<String> = vec![single_id.clone()];
    for entry in &entries {
        let id = entry["RecordId"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty(),
            "put record batch: every entry carries a RecordId, got {entry:?}"
        );
        assert!(
            entry.get("ErrorCode").is_none(),
            "put record batch: a stored record has no error, got {entry:?}"
        );
        assert!(
            !ids.contains(&id),
            "put record batch: RecordIds are distinct, {id} repeated in {batched:?}"
        );
        ids.push(id);
    }

    // Every id handed out names a record that really is in the buffer, in order.
    let buffered = emulator.buffered("demo-stream");
    let expected: Vec<(String, Vec<u8>)> = ids
        .iter()
        .cloned()
        .zip(
            std::iter::once("hello")
                .chain(batch.iter().copied())
                .map(|data| data.as_bytes().to_vec()),
        )
        .collect();
    assert_eq!(
        buffered, expected,
        "the buffer holds every put record, under the id its put answered, in arrival order"
    );

    // A put to a name no stream has is not found.
    let (status, missing) = emulator
        .call(
            "PutRecord",
            json!({
                "DeliveryStreamName": "no-such-stream",
                "Record": { "Data": aws_smithy_types::base64::encode("hello") },
            }),
        )
        .await;
    assert_eq!(status, 400, "put to an unknown stream: got {missing:?}");
    assert_eq!(
        error_code(&missing),
        "ResourceNotFoundException",
        "put to an unknown stream: got {missing:?}"
    );

    // DELETE is asynchronous too: the stream goes to DELETING and is still listed until
    // the clock removes it.
    let (status, deleted) = emulator
        .call(
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "demo-stream" }),
        )
        .await;
    assert_eq!(status, 200, "delete: got {deleted:?}");
    let (_, deleting) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "demo-stream" }),
        )
        .await;
    assert_eq!(
        deleting["DeliveryStreamDescription"]["DeliveryStreamStatus"],
        json!("DELETING"),
        "describe right after the delete: got {deleting:?}"
    );
    let (_, still_listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(
        still_listed["DeliveryStreamNames"],
        json!(["demo-stream"]),
        "list while deleting: a stream marked for deletion still exists, got {still_listed:?}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (status, gone) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "demo-stream" }),
        )
        .await;
    assert_eq!(status, 400, "describe after removal: got {gone:?}");
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "describe after removal: got {gone:?}"
    );
    let (_, emptied) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(
        emptied["DeliveryStreamNames"],
        json!([]),
        "list after removal: got {emptied:?}"
    );
    assert!(
        emulator.buffered("demo-stream").is_empty(),
        "the removed stream took its buffered data with it"
    );
}

/// One row per modelled destination kind: what a create supplies, and what the describe
/// must report back for it.
struct DestinationCase {
    stream: &'static str,
    config_member: &'static str,
    config: Value,
    description_member: &'static str,
    /// `(path inside the description, expected value)`.
    checks: Vec<(&'static str, Value)>,
    /// Credentials the create supplied: no describe may report them back.
    secrets: Vec<&'static str>,
}

/// A 256-character base64 body, the shortest `SnowflakePrivateKey` the model accepts.
pub(crate) fn snowflake_private_key() -> String {
    "A".repeat(256)
}

/// Every destination kind `DestinationDescription` models. The member lists come from
/// `smithy_query(query="shape", shape="DestinationDescription")` and the configuration
/// shapes it pairs with.
fn destination_cases() -> Vec<DestinationCase> {
    vec![
        DestinationCase {
            stream: "s3-stream",
            config_member: "S3DestinationConfiguration",
            config: s3_configuration(),
            description_member: "S3DestinationDescription",
            checks: vec![
                ("RoleARN", json!(ROLE_ARN)),
                ("BucketARN", json!(BUCKET_ARN)),
                ("Prefix", json!("raw/")),
                // The three members the description requires but the configuration leaves
                // optional take their documented defaults.
                ("BufferingHints/SizeInMBs", json!(5)),
                ("BufferingHints/IntervalInSeconds", json!(300)),
                ("CompressionFormat", json!("UNCOMPRESSED")),
                (
                    "EncryptionConfiguration/NoEncryptionConfig",
                    json!("NoEncryption"),
                ),
            ],
            secrets: vec![],
        },
        DestinationCase {
            stream: "extended-s3-stream",
            config_member: "ExtendedS3DestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "BucketARN": BUCKET_ARN,
                "CompressionFormat": "GZIP",
                "BufferingHints": { "SizeInMBs": 64, "IntervalInSeconds": 60 },
                "FileExtension": ".json",
            }),
            description_member: "ExtendedS3DestinationDescription",
            checks: vec![
                ("BucketARN", json!(BUCKET_ARN)),
                // Supplied values are reported, not replaced by the defaults.
                ("CompressionFormat", json!("GZIP")),
                ("BufferingHints/SizeInMBs", json!(64)),
                ("BufferingHints/IntervalInSeconds", json!(60)),
                ("FileExtension", json!(".json")),
            ],
            secrets: vec![],
        },
        DestinationCase {
            stream: "redshift-stream",
            config_member: "RedshiftDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterJDBCURL":
                    "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                "CopyCommand": { "DataTableName": "events" },
                "Username": "analytics",
                "Password": "sup3r-secret-pw",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "RedshiftDestinationDescription",
            checks: vec![
                ("CopyCommand/DataTableName", json!("events")),
                ("Username", json!("analytics")),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec!["sup3r-secret-pw"],
        },
        DestinationCase {
            stream: "elasticsearch-stream",
            config_member: "ElasticsearchDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
                "IndexName": "logs",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "ElasticsearchDestinationDescription",
            checks: vec![
                ("IndexName", json!("logs")),
                (
                    "DomainARN",
                    json!("arn:aws:es:us-west-2:000000000000:domain/logs-domain"),
                ),
                ("S3DestinationDescription/RoleARN", json!(ROLE_ARN)),
            ],
            secrets: vec![],
        },
        DestinationCase {
            stream: "opensearch-stream",
            config_member: "AmazonopensearchserviceDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "ClusterEndpoint": "https://search.example.com",
                "IndexName": "docs",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "AmazonopensearchserviceDestinationDescription",
            checks: vec![
                ("IndexName", json!("docs")),
                ("ClusterEndpoint", json!("https://search.example.com")),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec![],
        },
        DestinationCase {
            stream: "splunk-stream",
            config_member: "SplunkDestinationConfiguration",
            config: json!({
                "HECEndpoint": "https://splunk.example.com:8088",
                "HECEndpointType": "Event",
                "HECToken": "hec-token",
                "HECAcknowledgmentTimeoutInSeconds": 300,
                "S3Configuration": s3_configuration(),
            }),
            description_member: "SplunkDestinationDescription",
            checks: vec![
                ("HECEndpoint", json!("https://splunk.example.com:8088")),
                ("HECEndpointType", json!("Event")),
                ("HECAcknowledgmentTimeoutInSeconds", json!(300)),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec![],
        },
        DestinationCase {
            stream: "http-endpoint-stream",
            config_member: "HttpEndpointDestinationConfiguration",
            config: json!({
                "EndpointConfiguration": {
                    "Url": "https://example.com/firehose",
                    "Name": "example",
                    "AccessKey": "http-access-key",
                },
                "S3Configuration": s3_configuration(),
            }),
            description_member: "HttpEndpointDestinationDescription",
            checks: vec![
                (
                    "EndpointConfiguration/Url",
                    json!("https://example.com/firehose"),
                ),
                ("EndpointConfiguration/Name", json!("example")),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec!["http-access-key"],
        },
        DestinationCase {
            stream: "snowflake-stream",
            config_member: "SnowflakeDestinationConfiguration",
            config: json!({
                "AccountUrl": "my-account-id.snowflakecomputing.com",
                "PrivateKey": snowflake_private_key(),
                "KeyPassphrase": "passphrase",
                "User": "loader",
                "Database": "analytics",
                "Schema": "public",
                "Table": "events",
                "RoleARN": ROLE_ARN,
                "S3Configuration": s3_configuration(),
            }),
            description_member: "SnowflakeDestinationDescription",
            checks: vec![
                ("AccountUrl", json!("my-account-id.snowflakecomputing.com")),
                ("Database", json!("analytics")),
                ("Table", json!("events")),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec!["passphrase"],
        },
        DestinationCase {
            stream: "serverless-stream",
            config_member: "AmazonOpenSearchServerlessDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "CollectionEndpoint": "https://collection.example.com",
                "IndexName": "serverless-index",
                "S3Configuration": s3_configuration(),
            }),
            description_member: "AmazonOpenSearchServerlessDestinationDescription",
            checks: vec![
                ("IndexName", json!("serverless-index")),
                (
                    "CollectionEndpoint",
                    json!("https://collection.example.com"),
                ),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec![],
        },
        DestinationCase {
            stream: "iceberg-stream",
            config_member: "IcebergDestinationConfiguration",
            config: json!({
                "RoleARN": ROLE_ARN,
                "CatalogConfiguration": {
                    "CatalogARN": "arn:aws:glue:us-west-2:000000000000:catalog",
                },
                "AppendOnly": true,
                "S3Configuration": s3_configuration(),
            }),
            description_member: "IcebergDestinationDescription",
            checks: vec![
                ("RoleARN", json!(ROLE_ARN)),
                (
                    "CatalogConfiguration/CatalogARN",
                    json!("arn:aws:glue:us-west-2:000000000000:catalog"),
                ),
                ("AppendOnly", json!(true)),
                ("S3DestinationDescription/BucketARN", json!(BUCKET_ARN)),
            ],
            secrets: vec![],
        },
    ]
}

#[tokio::test]
async fn every_destination_kind_is_described_from_the_configuration_it_was_created_with() {
    let emulator = Emulator::new();

    for case in destination_cases() {
        let mut request = json!({ "DeliveryStreamName": case.stream });
        request[case.config_member] = case.config.clone();
        let (status, created) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {}: create with {} got {created:?}",
            case.stream, case.config_member
        );

        let (status, described) = emulator
            .call(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": case.stream }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {}: describe got {described:?}",
            case.stream
        );
        let destination = &described["DeliveryStreamDescription"]["Destinations"][0];
        // Compared as a SET: which members the body carries is the rule, and the order a
        // JSON object renders them in is not part of it.
        let mut reported: Vec<String> = destination
            .as_object()
            .map(|members| {
                members
                    .keys()
                    .filter(|key| key.as_str() != "DestinationId")
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        reported.sort();
        // Exactly the description matching the supplied configuration — plus, for a
        // destination whose own destination IS Amazon S3, the deprecated
        // `S3DestinationDescription` mirror Firehose reports beside the extended spelling
        // (`tests/all/legacy_s3_destination_mirror.rs` asserts the two agree). No other
        // kind's description is ever reported.
        let mut expected = vec![case.description_member.to_string()];
        if case.description_member == "ExtendedS3DestinationDescription" {
            expected.push("S3DestinationDescription".to_string());
        }
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {}: expected the descriptions {expected:?}, got {destination:?}",
            case.stream
        );

        let description = &destination[case.description_member];
        for (path, expected) in &case.checks {
            assert_eq!(
                at(description, path),
                expected,
                "case {}: {path} is {:?}, expected {expected:?}",
                case.stream,
                at(description, path)
            );
        }

        let rendered = described.to_string();
        for secret in &case.secrets {
            assert!(
                !rendered.contains(secret),
                "case {}: the credential supplied at create must not be reported back, \
                 but the describe body carries it: {rendered}",
                case.stream
            );
        }
    }
}

#[tokio::test]
async fn list_filters_by_type_pages_by_name_and_caps_by_limit() {
    let emulator = Emulator::new();
    // Inserted out of order: the list is alphabetical regardless.
    emulator
        .create_stream("gamma", Some("KinesisStreamAsSource"))
        .await;
    emulator.create_stream("alpha", None).await;
    emulator.create_stream("delta", Some("MSKAsSource")).await;
    emulator.create_stream("beta", Some("DirectPut")).await;

    // (case, request, expected names, expected HasMoreDeliveryStreams)
    let cases: Vec<(&str, Value, Value, bool)> = vec![
        (
            "no filter",
            json!({}),
            json!(["alpha", "beta", "delta", "gamma"]),
            false,
        ),
        ("limit under the count", json!({ "Limit": 2 }), json!(["alpha", "beta"]), true),
        (
            "limit at the count",
            json!({ "Limit": 4 }),
            json!(["alpha", "beta", "delta", "gamma"]),
            false,
        ),
        (
            "limit at the modelled maximum",
            json!({ "Limit": 10000 }),
            json!(["alpha", "beta", "delta", "gamma"]),
            false,
        ),
        (
            "exclusive start name",
            json!({ "ExclusiveStartDeliveryStreamName": "beta" }),
            json!(["delta", "gamma"]),
            false,
        ),
        (
            "exclusive start name past the last",
            json!({ "ExclusiveStartDeliveryStreamName": "zulu" }),
            json!([]),
            false,
        ),
        (
            "exclusive start name with a limit",
            json!({ "ExclusiveStartDeliveryStreamName": "beta", "Limit": 1 }),
            json!(["delta"]),
            true,
        ),
        (
            "type filter DirectPut",
            json!({ "DeliveryStreamType": "DirectPut" }),
            json!(["alpha", "beta"]),
            false,
        ),
        (
            "type filter KinesisStreamAsSource",
            json!({ "DeliveryStreamType": "KinesisStreamAsSource" }),
            json!(["gamma"]),
            false,
        ),
        (
            "type filter MSKAsSource",
            json!({ "DeliveryStreamType": "MSKAsSource" }),
            json!(["delta"]),
            false,
        ),
        (
            "type filter DatabaseAsSource",
            json!({ "DeliveryStreamType": "DatabaseAsSource" }),
            json!([]),
            false,
        ),
        (
            "type filter and paging together",
            json!({ "DeliveryStreamType": "DirectPut", "ExclusiveStartDeliveryStreamName": "alpha" }),
            json!(["beta"]),
            false,
        ),
    ];

    for (case, request, names, has_more) in cases {
        let (status, listed) = emulator.call("ListDeliveryStreams", request).await;
        assert_eq!(status, 200, "case {case:?}: got {listed:?}");
        assert_eq!(
            listed["DeliveryStreamNames"], names,
            "case {case:?}: expected {names:?}, got {:?}",
            listed["DeliveryStreamNames"]
        );
        assert_eq!(
            listed["HasMoreDeliveryStreams"],
            json!(has_more),
            "case {case:?}: expected HasMoreDeliveryStreams {has_more}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn list_without_a_limit_returns_the_documented_default_of_ten() {
    let emulator = Emulator::new();
    // `Limit`: "The maximum number of Firehose streams to list. The default value is 10."
    const DEFAULT_LIMIT: usize = 10;
    for index in 0..DEFAULT_LIMIT + 2 {
        emulator
            .create_stream(&format!("stream-{index:02}"), None)
            .await;
    }

    let (status, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(status, 200, "got {listed:?}");
    let names = listed["DeliveryStreamNames"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        names.len(),
        DEFAULT_LIMIT,
        "a list with no Limit returns the documented default of {DEFAULT_LIMIT}, got {listed:?}"
    );
    assert_eq!(
        names.first(),
        Some(&json!("stream-00")),
        "the default page starts at the first name, got {listed:?}"
    );
    assert_eq!(
        listed["HasMoreDeliveryStreams"],
        json!(true),
        "two names were left over, got {listed:?}"
    );
}

#[tokio::test]
async fn describe_pages_the_destination_list() {
    let emulator = Emulator::new();
    emulator.create_stream("paged-stream", None).await;

    // "You can have one destination per Firehose stream", so one destination is the whole
    // list: a limit that covers it leaves nothing more, and starting after it is empty.
    let cases: Vec<(&str, Value, usize, bool)> = vec![
        ("no paging", json!({}), 1, false),
        ("limit one", json!({ "Limit": 1 }), 1, false),
        ("limit at the modelled maximum", json!({ "Limit": 10000 }), 1, false),
        (
            "exclusive start at the only destination",
            json!({ "ExclusiveStartDestinationId": "destinationId-000000000001" }),
            0,
            false,
        ),
        (
            "exclusive start before the only destination",
            json!({ "ExclusiveStartDestinationId": "destinationId-000000000000" }),
            1,
            false,
        ),
    ];

    for (case, extra, expected_len, has_more) in cases {
        let mut request = json!({ "DeliveryStreamName": "paged-stream" });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            request[key] = value;
        }
        let (status, described) = emulator.call("DescribeDeliveryStream", request).await;
        assert_eq!(status, 200, "case {case:?}: got {described:?}");
        let description = &described["DeliveryStreamDescription"];
        let destinations = description["Destinations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            destinations.len(),
            expected_len,
            "case {case:?}: expected {expected_len} destinations, got {description:?}"
        );
        assert_eq!(
            description["HasMoreDestinations"],
            json!(has_more),
            "case {case:?}: expected HasMoreDestinations {has_more}, got {description:?}"
        );
    }
}

#[tokio::test]
async fn create_refuses_input_combinations_the_model_cannot_express() {
    let emulator = Emulator::new();
    let kms_key_arn = format!("arn:aws:kms:{REGION}:000000000000:key/demo-key");

    // (case, request body, expected error code)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no destination configuration at all",
            json!({ "DeliveryStreamName": "no-destination" }),
            "InvalidArgumentException",
        ),
        (
            "two destination configurations",
            json!({
                "DeliveryStreamName": "two-destinations",
                "S3DestinationConfiguration": s3_configuration(),
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                },
            }),
            "InvalidArgumentException",
        ),
        (
            "a Kinesis source on a DirectPut stream",
            json!({
                "DeliveryStreamName": "mismatched-source",
                "DeliveryStreamType": "DirectPut",
                "S3DestinationConfiguration": s3_configuration(),
                "KinesisStreamSourceConfiguration": {
                    "KinesisStreamARN": format!("arn:aws:kinesis:{REGION}:000000000000:stream/src"),
                    "RoleARN": ROLE_ARN,
                },
            }),
            "InvalidArgumentException",
        ),
        (
            "KinesisStreamAsSource without its source configuration",
            json!({
                "DeliveryStreamName": "missing-source",
                "DeliveryStreamType": "KinesisStreamAsSource",
                "S3DestinationConfiguration": s3_configuration(),
            }),
            "InvalidArgumentException",
        ),
        (
            "MSKAsSource without its source configuration",
            json!({
                "DeliveryStreamName": "missing-msk-source",
                "DeliveryStreamType": "MSKAsSource",
                "S3DestinationConfiguration": s3_configuration(),
            }),
            "InvalidArgumentException",
        ),
        (
            "a customer managed CMK without its key ARN",
            json!({
                "DeliveryStreamName": "cmk-without-arn",
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": { "KeyType": "CUSTOMER_MANAGED_CMK" },
            }),
            "InvalidArgumentException",
        ),
        (
            "an AWS owned CMK with a key ARN",
            json!({
                "DeliveryStreamName": "owned-cmk-with-arn",
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "AWS_OWNED_CMK",
                    "KeyARN": kms_key_arn,
                },
            }),
            "InvalidArgumentException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, refused) = emulator.call("CreateDeliveryStream", request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            expected,
            "case {case:?}: got {refused:?}"
        );
    }

    // A CMK in another Region cannot be used, and the model has its own error for a KMS
    // resource Firehose cannot reach.
    let (status, wrong_region) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "cmk-in-another-region",
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "CUSTOMER_MANAGED_CMK",
                    "KeyARN": "arn:aws:kms:eu-west-1:000000000000:key/demo-key",
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a CMK in another Region: got {wrong_region:?}"
    );
    assert_eq!(
        error_code(&wrong_region),
        "InvalidKMSResourceException",
        "a CMK in another Region: got {wrong_region:?}"
    );

    // Nothing was stored for any refused create.
    let (_, listed) = emulator.call("ListDeliveryStreams", json!({})).await;
    assert_eq!(
        listed["DeliveryStreamNames"],
        json!([]),
        "a refused create stores nothing, got {listed:?}"
    );
}

#[tokio::test]
async fn an_operation_on_a_stream_in_the_wrong_state_is_refused() {
    let emulator = Emulator::new();

    // (case, operation, request, expected error code)
    let unknown: Vec<(&str, &str, Value, &str)> = vec![
        (
            "describe an unknown stream",
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "ghost" }),
            "ResourceNotFoundException",
        ),
        (
            "delete an unknown stream",
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "ghost" }),
            "ResourceNotFoundException",
        ),
        (
            "put a record to an unknown stream",
            "PutRecord",
            json!({
                "DeliveryStreamName": "ghost",
                "Record": { "Data": aws_smithy_types::base64::encode("x") },
            }),
            "ResourceNotFoundException",
        ),
        (
            "put a batch to an unknown stream",
            "PutRecordBatch",
            json!({
                "DeliveryStreamName": "ghost",
                "Records": [{ "Data": aws_smithy_types::base64::encode("x") }],
            }),
            "ResourceNotFoundException",
        ),
    ];
    for (case, operation, request, expected) in unknown {
        let (status, refused) = emulator.call(operation, request).await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            expected,
            "case {case:?}: got {refused:?}"
        );
    }

    // "You can't delete a Firehose stream that is in the CREATING state."
    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "fresh-stream",
                "S3DestinationConfiguration": s3_configuration(),
            }),
        )
        .await;
    assert_eq!(status, 200, "create: got {created:?}");
    let (status, refused) = emulator
        .call(
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "fresh-stream" }),
        )
        .await;
    assert_eq!(status, 400, "delete while creating: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "delete while creating: got {refused:?}"
    );

    // Once it is ACTIVE the same delete succeeds, and a second one is accepted without
    // postponing or failing the removal already under way.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    for attempt in 1..=2 {
        let (status, deleted) = emulator
            .call(
                "DeleteDeliveryStream",
                json!({ "DeliveryStreamName": "fresh-stream" }),
            )
            .await;
        assert_eq!(status, 200, "delete attempt {attempt}: got {deleted:?}");
    }
}

#[tokio::test]
async fn a_record_list_outside_its_modelled_bounds_is_refused() {
    let emulator = Emulator::new();
    emulator.create_stream("bounded-stream", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    let one_record = json!({ "Data": aws_smithy_types::base64::encode("x") });
    // `Records`: a required list of length 1..500.
    let cases: Vec<(&str, Value)> = vec![
        ("an empty record list", json!([])),
        (
            "one record over the maximum",
            Value::Array(vec![one_record.clone(); 501]),
        ),
    ];

    for (case, records) in cases {
        let (status, refused) = emulator
            .call(
                "PutRecordBatch",
                json!({ "DeliveryStreamName": "bounded-stream", "Records": records }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        // The 1..500 bound is modelled, so the boundary validator refuses the request
        // before the handler runs, with the code it uses for a constraint violation on
        // awsJson1_1 (`crate::scaffolded::Scaffolded::ValidationException`) — never a 501.
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: a request outside the modelled bound is refused as invalid, \
             got {refused:?}"
        );
    }

    // The batch at the maximum is accepted, and every record of it is buffered.
    let records = Value::Array(vec![one_record; 500]);
    let (status, accepted) = emulator
        .call(
            "PutRecordBatch",
            json!({ "DeliveryStreamName": "bounded-stream", "Records": records }),
        )
        .await;
    assert_eq!(status, 200, "a batch at the maximum: got {accepted:?}");
    assert_eq!(
        accepted["RequestResponses"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        500,
        "a batch at the maximum answers one entry per record, got {accepted:?}"
    );
    assert_eq!(
        emulator.buffered("bounded-stream").len(),
        500,
        "every record of the accepted batch is buffered"
    );
}

#[tokio::test]
async fn ingestion_stops_at_the_buffer_budget_instead_of_growing_without_bound() {
    let emulator = Emulator::new();
    emulator.create_stream("budgeted-stream", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // The budget is the emulator's own (it holds buffered records in this process's
    // heap), not an AWS limit, so the test reads it from the state module rather than
    // hard-coding a number, and each record is the largest the model allows.
    const RECORD_BYTES: usize = 1_024_000;
    let data = aws_smithy_types::base64::encode(vec![b'x'; RECORD_BYTES]);
    let fits = service_com_amazonaws_firehose_firehose_20150804::state::MAX_BUFFERED_BYTES_PER_STREAM
        / RECORD_BYTES;

    for put in 1..=fits {
        let (status, accepted) = emulator
            .call(
                "PutRecord",
                json!({ "DeliveryStreamName": "budgeted-stream", "Record": { "Data": data } }),
            )
            .await;
        assert_eq!(status, 200, "put {put} of {fits} within budget: got {accepted:?}");
    }
    assert_eq!(
        emulator.buffered("budgeted-stream").len(),
        fits,
        "every put within the budget is buffered"
    );

    // The next one does not fit: refused with the modelled 503, and nothing stored.
    let (status, refused) = emulator
        .call(
            "PutRecord",
            json!({ "DeliveryStreamName": "budgeted-stream", "Record": { "Data": data } }),
        )
        .await;
    assert_eq!(status, 503, "the put past the budget: got {refused:?}");
    assert_eq!(
        error_code(&refused),
        "ServiceUnavailableException",
        "the put past the budget: got {refused:?}"
    );
    assert_eq!(
        emulator.buffered("budgeted-stream").len(),
        fits,
        "a refused put stores nothing"
    );

    // A batch at the budget is reported per record: no RecordId, an ErrorCode, and a
    // FailedPutCount that counts them.
    let (status, batched) = emulator
        .call(
            "PutRecordBatch",
            json!({
                "DeliveryStreamName": "budgeted-stream",
                "Records": [{ "Data": data }, { "Data": data }],
            }),
        )
        .await;
    assert_eq!(status, 200, "a batch past the budget: got {batched:?}");
    assert_eq!(
        batched["FailedPutCount"],
        json!(2),
        "a batch past the budget: got {batched:?}"
    );
    for entry in batched["RequestResponses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        assert_eq!(
            entry["ErrorCode"],
            json!("ServiceUnavailableException"),
            "a batch past the budget: got {entry:?}"
        );
        assert!(
            entry.get("RecordId").is_none(),
            "a record that was not stored gets no RecordId, got {entry:?}"
        );
    }
    assert_eq!(
        emulator.buffered("budgeted-stream").len(),
        fits,
        "a refused batch stores nothing"
    );
}

#[tokio::test]
async fn a_stream_created_with_encryption_reports_it_and_marks_its_records_encrypted() {
    let emulator = Emulator::new();
    let key_arn = format!("arn:aws:kms:{REGION}:000000000000:key/demo-key");

    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "encrypted-stream",
                "S3DestinationConfiguration": s3_configuration(),
                "DeliveryStreamEncryptionConfigurationInput": {
                    "KeyType": "CUSTOMER_MANAGED_CMK",
                    "KeyARN": key_arn,
                },
            }),
        )
        .await;
    assert_eq!(status, 200, "create with SSE: got {created:?}");

    // Enabling SSE is asynchronous, like the creation it rides along with.
    let (_, enabling) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "encrypted-stream" }),
        )
        .await;
    let encryption =
        &enabling["DeliveryStreamDescription"]["DeliveryStreamEncryptionConfiguration"];
    assert_eq!(
        encryption["Status"],
        json!("ENABLING"),
        "describe while creating: got {encryption:?}"
    );
    assert_eq!(
        encryption["KeyType"],
        json!("CUSTOMER_MANAGED_CMK"),
        "describe while creating: got {encryption:?}"
    );
    assert_eq!(
        encryption["KeyARN"],
        json!(key_arn),
        "describe while creating: got {encryption:?}"
    );

    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (_, enabled) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "encrypted-stream" }),
        )
        .await;
    assert_eq!(
        enabled["DeliveryStreamDescription"]["DeliveryStreamEncryptionConfiguration"]["Status"],
        json!("ENABLED"),
        "describe after the enabling span: got {enabled:?}"
    );

    // With SSE on, a put reports the record as encrypted; the unencrypted stream of the
    // lifecycle test reports false for the same call.
    let (status, put) = emulator
        .call(
            "PutRecord",
            json!({
                "DeliveryStreamName": "encrypted-stream",
                "Record": { "Data": aws_smithy_types::base64::encode("hello") },
            }),
        )
        .await;
    assert_eq!(status, 200, "put to the encrypted stream: got {put:?}");
    assert_eq!(
        put["Encrypted"],
        json!(true),
        "put to the encrypted stream: got {put:?}"
    );
}

#[tokio::test]
async fn a_kinesis_sourced_stream_reports_its_source() {
    let emulator = Emulator::new();
    let stream_arn = format!("arn:aws:kinesis:{REGION}:000000000000:stream/source-stream");

    let (status, created) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": "kinesis-sourced",
                "DeliveryStreamType": "KinesisStreamAsSource",
                "KinesisStreamSourceConfiguration": {
                    "KinesisStreamARN": stream_arn,
                    "RoleARN": ROLE_ARN,
                },
                "S3DestinationConfiguration": s3_configuration(),
            }),
        )
        .await;
    assert_eq!(status, 200, "create: got {created:?}");

    let (_, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "kinesis-sourced" }),
        )
        .await;
    let description = &described["DeliveryStreamDescription"];
    assert_eq!(
        description["DeliveryStreamType"],
        json!("KinesisStreamAsSource"),
        "describe: got {description:?}"
    );
    assert_eq!(
        at(description, "Source/KinesisStreamSourceDescription/KinesisStreamARN"),
        &json!(stream_arn),
        "describe: the source names the Kinesis stream the create supplied, got \
         {description:?}"
    );
    assert_eq!(
        at(description, "Source/KinesisStreamSourceDescription/RoleARN"),
        &json!(ROLE_ARN),
        "describe: got {description:?}"
    );
}
