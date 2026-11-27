// SPDX-License-Identifier: CC0-1.0

//! Where an Amazon MSK cluster's BROKER LOGS go: exactly the destinations its own
//! `LoggingInfo.BrokerLogs` enables and names, and nowhere else.
//!
//! RULE under test: `BrokerLogs` carries three destination kinds, each gated by its own
//! required `Enabled` — a CloudWatch Logs `LogGroup`, an Amazon S3 `Bucket`/`Prefix` and a
//! Firehose `DeliveryStream`. A provisioned cluster delivers the Apache Kafka log lines its
//! brokers write to EXACTLY the destinations that are both enabled and named: the log group
//! receives them as log events, the bucket receives them as a non-empty object under the
//! declared prefix, and a destination that is not enabled, one that is enabled without
//! naming where to write, one naming a group the account does not hold, and a cluster
//! carrying no `LoggingInfo` at all each receive nothing. The destinations are the
//! ACCOUNT's, so a log group nothing created stays absent rather than being created by the
//! delivery.
//!
//! The lines themselves are the BROKER's: each is Apache Kafka's own log4j layout — a level
//! token and the `kafka.…` logger that wrote it — reporting what the cluster's brokers really
//! did, the nodes coming up when the cluster reached `ACTIVE` and each partition log of a
//! created topic.
//!
//! Only a cross-service suite can pin this: the flow spans three services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — Amazon MSK for the cluster that declares
//! the destinations and writes the lines, CloudWatch Logs for the group the events are read
//! out of, and Amazon S3 for the object a reader fetches.
//!
//! The rows differ along what the rule generalises over: which destination kinds the
//! `BrokerLogs` carries at all, which of them are enabled, whether an enabled one names a
//! place to write (a held group, a group nothing created, no group at all), whether the
//! bucket destination declares a prefix, and a cluster that declares no logging at all.
//!
//! Every wait LOOKS AGAIN until its condition holds on a generous deadline, and every
//! assertion that something stays ABSENT looks repeatedly: nothing here is asserted about
//! elapsed time.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::elements;

/// The Region every call below is signed for, and the one every ARN here names.
const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN and delivered key carries
/// (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The `X-Amz-Target` prefix CloudWatch Logs' awsJson wire carries.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The Apache Kafka version and broker type every cluster here is created with: a version
/// Amazon MSK still takes for a new cluster, since the create has to be admitted.
const KAFKA_VERSION: &str = "3.9.x";
const INSTANCE_TYPE: &str = "kafka.m5.large";

/// The two client subnets every cluster's brokers are placed in, one broker each.
const SUBNETS: [&str; 2] = ["subnet-0a1b2c3d", "subnet-1b2c3d4e"];

/// The topic every row creates on its cluster, and how many partitions it asks for: each
/// partition's log is one more line the broker writes.
const TOPIC: &str = "broker-log-topic";
const PARTITIONS: i64 = 3;

/// The root segment every delivered log file of an AWS service sits under in a bucket.
const DELIVERED_LOGS_SEGMENT: &str = "AWSLogs";

/// Amazon MSK's own segment under that root, which a delivered broker-log object sits in.
const BROKER_LOGS_SEGMENT: &str = "KafkaBrokerLogs";

/// The first two bytes of a gzip stream (RFC 1952's identification fields), which is what a
/// delivered log file IS.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// How long a wait for something the emulator's clock owes is given before a row fails, and
/// how often it looks again. Generous rather than tight: a row whose work is done spends
/// none of it.
const DEADLINE: Duration = Duration::from_secs(60);
/// See [`DEADLINE`].
const PROBE: Duration = Duration::from_millis(50);

/// How many times an assertion that something stays ABSENT looks, with [`PROBE`] between
/// looks: the span is spent only by the rows that assert an absence.
const QUIET_LOOKS: usize = 8;

/// Amazon MSK, plus the two services its declared destinations belong to. One engine, as
/// `core/router/src/lib.rs` mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// The `Authorization` header every call carries. Its signature is never checked: this
/// router runs with authentication off, and the header is there so the Region and the route
/// key resolve.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One request to the service `signing_name`; answers its status and body bytes.
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

/// One Amazon MSK call, over the `restJson1` wire that service serves.
async fn kafka(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let (status, answered) = send(
        router,
        "kafka",
        method,
        path,
        &[("content-type", "application/json")],
        payload,
    )
    .await;
    let parsed = serde_json::from_slice(&answered).unwrap_or(Value::Null);
    (status, parsed)
}

/// One CloudWatch Logs call, over the `awsJson` wire that service serves.
async fn logs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let (status, answered) = send(
        router,
        "logs",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &format!("{LOGS_TARGET_PREFIX}.{operation}")),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    let parsed = serde_json::from_slice(&answered).unwrap_or(Value::Null);
    (status, parsed)
}

/// One ARN as a single non-greedy `@httpLabel` path segment, as an SDK sends it.
fn label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// Which log group a row's `cloudWatchLogs` destination names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// The group this row created and wired the cluster to.
    Wired,
    /// A well-formed group name nothing created, so the account holds no such group.
    Absent,
    /// No `logGroup` member at all: the destination names nowhere to write.
    Unnamed,
}

/// One row: the `BrokerLogs` a cluster is created with, and what each destination must then
/// hold.
struct Row {
    case: &'static str,
    /// What this row's cluster, bucket and log groups are named after.
    name: &'static str,
    /// The `cloudWatchLogs` member: `None` leaves it out of `brokerLogs` entirely.
    cloud_watch: Option<(bool, Group)>,
    /// The `s3` member: `(enabled, prefix)`; `None` leaves it out entirely.
    s3: Option<(bool, Option<&'static str>)>,
    /// The `firehose` member's `enabled`; `None` leaves it out entirely.
    firehose: Option<bool>,
    /// Whether the WIRED log group must hold the brokers' lines.
    expect_events: bool,
    /// Whether the bucket must hold a delivered object.
    expect_object: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded shape: both an enabled group and an enabled bucket, with
            // Firehose explicitly off.
            case: "an enabled group and an enabled bucket under a prefix, firehose disabled",
            name: "both",
            cloud_watch: Some((true, Group::Wired)),
            s3: Some((true, Some("msk/broker-logs"))),
            firehose: Some(false),
            expect_events: true,
            expect_object: true,
        },
        Row {
            case: "only the group enabled, the bucket destination off",
            name: "group-only",
            cloud_watch: Some((true, Group::Wired)),
            s3: Some((false, Some("never-written"))),
            firehose: None,
            expect_events: true,
            expect_object: false,
        },
        Row {
            case: "only the bucket enabled, naming no prefix at all",
            name: "bucket-only",
            cloud_watch: Some((false, Group::Wired)),
            s3: Some((true, None)),
            firehose: Some(false),
            expect_events: false,
            expect_object: true,
        },
        Row {
            case: "an enabled group that names no group, beside an enabled bucket",
            name: "unnamed-group",
            cloud_watch: Some((true, Group::Unnamed)),
            s3: Some((true, Some("unnamed"))),
            firehose: None,
            expect_events: false,
            expect_object: true,
        },
        Row {
            case: "an enabled group the account does not hold, beside an enabled bucket",
            name: "absent-group",
            cloud_watch: Some((true, Group::Absent)),
            s3: Some((true, Some("absent"))),
            firehose: None,
            expect_events: false,
            expect_object: true,
        },
        Row {
            case: "a cluster that declares no loggingInfo at all",
            name: "no-logging",
            cloud_watch: None,
            s3: None,
            firehose: None,
            expect_events: false,
            expect_object: false,
        },
    ]
}

impl Row {
    /// The group this row's cluster is wired to, and which a reader looks for its lines in.
    fn wired_group(&self) -> String {
        format!("msk-broker-logs-{}-wired", self.name)
    }

    /// A second group of the account's, deliberately left out of every `LoggingInfo`: it
    /// must receive nothing.
    fn unwired_group(&self) -> String {
        format!("msk-broker-logs-{}-unwired", self.name)
    }

    /// A group name nothing creates, which the `Group::Absent` destination names.
    fn absent_group(&self) -> String {
        format!("msk-broker-logs-{}-nothing-created-this", self.name)
    }

    fn bucket(&self) -> String {
        format!("msk-broker-logs-{}", self.name)
    }

    fn cluster(&self) -> String {
        format!("msk-broker-logs-{}", self.name)
    }

    /// Whether this row's `brokerLogs` carries any member at all; one that carries none
    /// sends no `loggingInfo`.
    fn declares_logging(&self) -> bool {
        self.cloud_watch.is_some() || self.s3.is_some() || self.firehose.is_some()
    }

    /// The `loggingInfo` this row's `CreateCluster` carries, or `None`.
    fn logging_info(&self) -> Option<Value> {
        if !self.declares_logging() {
            return None;
        }
        let mut broker_logs = json!({});
        if let Some((enabled, group)) = self.cloud_watch {
            let mut destination = json!({ "enabled": enabled });
            match group {
                Group::Wired => destination["logGroup"] = json!(self.wired_group()),
                Group::Absent => destination["logGroup"] = json!(self.absent_group()),
                Group::Unnamed => {}
            }
            broker_logs["cloudWatchLogs"] = destination;
        }
        if let Some((enabled, prefix)) = self.s3 {
            let mut destination = json!({ "enabled": enabled, "bucket": self.bucket() });
            if let Some(prefix) = prefix {
                destination["prefix"] = json!(prefix);
            }
            broker_logs["s3"] = destination;
        }
        if let Some(enabled) = self.firehose {
            broker_logs["firehose"] = json!({ "enabled": enabled });
        }
        Some(json!({ "brokerLogs": broker_logs }))
    }

    /// The key prefix a delivered object of this row must sit under: the destination's own
    /// prefix, then the layout every delivered log file shares.
    fn delivered_prefix(&self) -> String {
        let declared = match self.s3 {
            Some((_, Some(prefix))) => format!("{prefix}/"),
            _ => String::new(),
        };
        format!("{declared}{DELIVERED_LOGS_SEGMENT}/{ACCOUNT}/{BROKER_LOGS_SEGMENT}/{REGION}/")
    }
}

/// Create the CloudWatch Logs group `name`.
async fn given_log_group(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, answered) = logs(router, "CreateLogGroup", &json!({ "logGroupName": name })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the log group {name:?} must be created, got {status} {answered}"
    );
}

/// Create the destination bucket `name`.
async fn given_bucket(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, answered) = send(
        router,
        "s3",
        "PUT",
        &format!("/{name}"),
        &[],
        format!(
            "<CreateBucketConfiguration><LocationConstraint>{REGION}\
             </LocationConstraint></CreateBucketConfiguration>"
        )
        .into_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: the destination bucket {name:?} must be created, got {status} {}",
        String::from_utf8_lossy(&answered)
    );
}

/// Every log event the group `name` holds, as `FilterLogEvents` reports them: the message of
/// each, with the stream it is in.
async fn events_in(router: &MultiServiceRouter, name: &str) -> Vec<(String, String)> {
    let (status, answered) = logs(router, "FilterLogEvents", &json!({ "logGroupName": name })).await;
    if status != 200 {
        return Vec::new();
    }
    answered
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .map(|event| {
                    (
                        event
                            .get("logStreamName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        event
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The log streams the group `name` holds, as `DescribeLogStreams` reports them.
async fn streams_in(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let (status, answered) = logs(
        router,
        "DescribeLogStreams",
        &json!({ "logGroupName": name }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    answered
        .get("logStreams")
        .and_then(Value::as_array)
        .map(|streams| {
            streams
                .iter()
                .filter_map(|stream| stream.get("logStreamName").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whether CloudWatch Logs holds a group of exactly the name `name`.
async fn holds_group(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, answered) = logs(
        router,
        "DescribeLogGroups",
        &json!({ "logGroupIdentifiers": [name] }),
    )
    .await;
    status == 200
        && answered
            .get("logGroups")
            .and_then(Value::as_array)
            .is_some_and(|groups| {
                groups.iter().any(|group| {
                    group.get("logGroupName").and_then(Value::as_str) == Some(name)
                })
            })
}

/// Every key the bucket `bucket` holds, read through `ListObjectsV2`.
async fn keys_in(router: &MultiServiceRouter, bucket: &str) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{bucket}?list-type=2"),
        &[],
        Vec::new(),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    let mut keys = elements(&String::from_utf8_lossy(&body), "Key");
    keys.sort();
    keys
}

/// The bytes of the object `key` of `bucket`.
async fn object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str) -> Vec<u8> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{bucket}/{key}"),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "case {case:?}: the delivered object {key:?} must be fetchable, got {status} {}",
        String::from_utf8_lossy(&body)
    );
    body
}

/// Whether `line` is one of the broker's own log lines: Apache Kafka's log4j layout, with a
/// level token and the `kafka.…` logger that wrote it.
fn is_broker_log_line(line: &str) -> bool {
    let levels = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR", "FATAL"];
    line.starts_with('[')
        && levels.iter().any(|level| line.contains(level))
        && line.contains("(kafka.")
        && line.trim_end().ends_with(')')
}

/// The Apache Kafka api keys a client of this endpoint has to find advertised to drive the
/// chain the broker logs are written by: `Produce`, `Metadata`, `CreateTopics` and
/// `DescribeConfigs`, as the Apache Kafka protocol numbers them.
const REQUIRED_API_KEYS: [i16; 4] = [0, 3, 19, 32];

/// The api key of `ApiVersions` itself, and the version of it framed below.
const API_VERSIONS_KEY: i16 = 18;
const API_VERSIONS_VERSION: i16 = 0;

/// What one `ApiVersions` exchange on a published bootstrap address answered.
struct Negotiated {
    error_code: i16,
    api_keys: Vec<i16>,
}

/// The loopback interface the core binds every data-plane socket on, and therefore the only
/// host a published broker address names.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// The PORT of the published `host:port` `address`, with the host asserted to be this
/// machine's own loopback interface — the only address this emulator opens a broker socket
/// on, so a published pair naming anything else is one no client could reach.
fn loopback_port(case: &str, address: &str) -> u16 {
    let (host, port) = address.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: a published broker address is a host:port pair, got {address:?}")
    });
    assert_eq!(
        host, LOOPBACK_HOST,
        "case {case:?}: a published broker address names this emulator's own loopback \
         interface, got {address:?}"
    );
    port.parse().unwrap_or_else(|error| {
        panic!("case {case:?}: {address:?} must name the port the core assigned, got {error}")
    })
}

/// Open a connection to `address` — a `host:port` the control plane published — and ask it
/// for the APIs it serves, framing the request and reading the answer by hand, as a client
/// that speaks the Apache Kafka protocol does.
///
/// Nothing leaves this machine: the port comes from the address the control plane published
/// and the host is this emulator's own loopback interface ([`loopback_port`]).
///
/// The exchange is BLOCKING and runs on the blocking pool, so the runtime thread serving the
/// broker's own end of the connection stays free to answer it.
async fn api_versions(case: &str, address: &str) -> Negotiated {
    let port = loopback_port(case, address);
    let case = case.to_string();
    tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap_or_else(
            |error| panic!("case {case:?}: 127.0.0.1:{port} must accept a connection, got {error}"),
        );
        let client_id = b"msk-broker-log-probe";
        let mut frame: Vec<u8> = Vec::new();
        frame.extend_from_slice(&API_VERSIONS_KEY.to_be_bytes());
        frame.extend_from_slice(&API_VERSIONS_VERSION.to_be_bytes());
        frame.extend_from_slice(&1i32.to_be_bytes());
        frame.extend_from_slice(&i16::try_from(client_id.len()).unwrap_or(0).to_be_bytes());
        frame.extend_from_slice(client_id);
        let mut request: Vec<u8> =
            i32::try_from(frame.len()).unwrap_or(0).to_be_bytes().to_vec();
        request.extend_from_slice(&frame);
        stream
            .write_all(&request)
            .unwrap_or_else(|error| panic!("case {case:?}: the request must be sent, got {error}"));

        let read_exact = |count: usize, stream: &mut std::net::TcpStream| -> Vec<u8> {
            let mut taken = vec![0u8; count];
            stream.read_exact(&mut taken).unwrap_or_else(|error| {
                panic!("case {case:?}: the broker must answer {count} bytes, got {error}")
            });
            taken
        };
        let length = read_exact(4, &mut stream);
        let length = i32::from_be_bytes([length[0], length[1], length[2], length[3]]);
        let body = read_exact(usize::try_from(length).unwrap_or(0), &mut stream);
        // The `ApiVersions` version 0 answer: the correlation id of the response header,
        // then the error code and the array of the APIs the broker serves.
        let error_code = i16::from_be_bytes([body[4], body[5]]);
        let count = i32::from_be_bytes([body[6], body[7], body[8], body[9]]);
        let api_keys = (0..usize::try_from(count).unwrap_or(0))
            .filter_map(|index| {
                let at = 10 + index * 6;
                Some(i16::from_be_bytes([
                    *body.get(at)?,
                    *body.get(at + 1)?,
                ]))
            })
            .collect();
        Negotiated {
            error_code,
            api_keys,
        }
    })
    .await
    .expect("the probe runs to completion")
}

#[tokio::test]
async fn broker_logs_reach_exactly_the_destinations_logging_info_names() {
    let router = emulator();
    let rows = rows();

    // Every row's destinations first, so each cluster is created with the groups and the
    // bucket it declares already there — which is the position a client is in.
    for row in &rows {
        given_log_group(&router, row.case, &row.wired_group()).await;
        given_log_group(&router, row.case, &row.unwired_group()).await;
        given_bucket(&router, row.case, &row.bucket()).await;
    }

    // Then every row's cluster, created together so one wait covers the spans the creates
    // armed.
    let mut created: Vec<String> = Vec::new();
    for row in &rows {
        let mut body = json!({
            "clusterName": row.cluster(),
            "kafkaVersion": KAFKA_VERSION,
            "numberOfBrokerNodes": SUBNETS.len(),
            "brokerNodeGroupInfo": {
                "instanceType": INSTANCE_TYPE,
                "clientSubnets": SUBNETS,
            },
            // The recorded shape: clients reach these brokers in the clear, unauthenticated.
            "encryptionInfo": {
                "encryptionInTransit": { "clientBroker": "PLAINTEXT", "inCluster": true },
            },
            "clientAuthentication": { "unauthenticated": { "enabled": true } },
        });
        if let Some(logging) = row.logging_info() {
            body["loggingInfo"] = logging;
        }
        let (status, answered) = kafka(&router, "POST", "/v1/clusters", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateCluster must be accepted, got {status} {answered}",
            row.case
        );
        let arn = answered
            .get("clusterArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the create must report a clusterArn, got {answered}",
                    row.case
                )
            })
            .to_string();
        let state = answered
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            state == "CREATING" || state == "ACTIVE",
            "case {:?}: the create must report CREATING or ACTIVE, got {state:?}",
            row.case
        );
        created.push(arn);
    }

    for (row, cluster_arn) in rows.iter().zip(&created) {
        let case = row.case;
        // The cluster reaches ACTIVE, and reports back exactly the LoggingInfo it was
        // created with: the destinations a reader sees are the ones the delivery uses.
        let described = until_active(&router, case, cluster_arn).await;
        assert_eq!(
            described.pointer("/clusterInfo/provisioned/loggingInfo").cloned(),
            row.logging_info(),
            "case {case:?}: DescribeClusterV2 must report the declared LoggingInfo, got \
             {described}"
        );

        // And a topic is created on it, which is one more thing its brokers write about.
        let (status, answered) = kafka(
            &router,
            "POST",
            &format!("/v1/clusters/{}/topics", label(cluster_arn)),
            Some(json!({
                "topicName": TOPIC,
                "partitionCount": PARTITIONS,
                "replicationFactor": SUBNETS.len(),
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTopic must be accepted, got {status} {answered}"
        );

        // THE GROUP LEG.
        let wired = row.wired_group();
        if row.expect_events {
            let events = until_events(&router, case, &wired).await;
            // Every event is one of the broker's own log lines, in Apache Kafka's layout.
            for (stream, message) in &events {
                assert!(
                    is_broker_log_line(message),
                    "case {case:?}: every delivered event must be an Apache Kafka broker log \
                     line with a level token and a kafka logger, got {message:?}"
                );
                assert!(
                    stream.starts_with(&row.cluster()) && stream.contains("broker-"),
                    "case {case:?}: a broker's lines go in a stream naming its cluster and \
                     itself, got {stream:?}"
                );
            }
            // Both the lines the clock owed the cluster (its brokers coming up) and the ones
            // its own topic create produced are there, so the delivery carries what the
            // brokers really did rather than one canned line.
            let messages: Vec<&str> = events
                .iter()
                .map(|(_, message)| message.as_str())
                .collect();
            assert!(
                messages.iter().any(|message| message.contains("started")),
                "case {case:?}: the brokers coming up must be in the group, got {messages:?}"
            );
            assert!(
                messages
                    .iter()
                    .any(|message| message.contains(TOPIC) && message.contains("Created log")),
                "case {case:?}: the topic's partition logs must be in the group, got \
                 {messages:?}"
            );
            // One stream per broker node of the cluster: each broker's lines are its own.
            let mut streams = streams_in(&router, &wired).await;
            streams.sort();
            streams.dedup();
            assert_eq!(
                streams.len(),
                SUBNETS.len(),
                "case {case:?}: expected one stream per broker node, got {streams:?}"
            );
        } else {
            assert_stays_empty(&router, case, &wired, "the group the cluster declares").await;
        }
        // A group name nothing created stays absent: a delivery writes into the account's
        // own group and creates none.
        if row.cloud_watch == Some((true, Group::Absent)) {
            let absent = row.absent_group();
            assert!(
                !holds_group(&router, &absent).await,
                "case {case:?}: the delivery must not create the group {absent:?}"
            );
            assert_stays_empty(&router, case, &absent, "a group nothing created").await;
        }
        // And the group the cluster was never wired to receives nothing at all, whatever
        // the row delivers elsewhere.
        assert_stays_empty(&router, case, &row.unwired_group(), "the unwired group").await;

        // THE BUCKET LEG.
        let bucket = row.bucket();
        if row.expect_object {
            let keys = until_objects(&router, case, &bucket).await;
            let prefix = row.delivered_prefix();
            for key in &keys {
                assert!(
                    key.starts_with(&prefix),
                    "case {case:?}: a delivered object sits under {prefix:?}, got {key:?}"
                );
                assert!(
                    key.contains(&row.cluster()) && key.ends_with(".log.gz"),
                    "case {case:?}: a delivered log file names its cluster and is a log file, \
                     got {key:?}"
                );
            }
            let body = object(&router, case, &bucket, &keys[0]).await;
            assert!(
                !body.is_empty(),
                "case {case:?}: the delivered object {:?} must not be empty",
                keys[0]
            );
            assert_eq!(
                body.get(..2),
                Some(GZIP_MAGIC.as_slice()),
                "case {case:?}: a delivered log file is a gzip stream, got the first bytes {:?}",
                body.get(..2)
            );
        } else {
            for _ in 0..QUIET_LOOKS {
                let keys = keys_in(&router, &bucket).await;
                assert!(
                    keys.is_empty(),
                    "case {case:?}: a bucket destination that is not enabled must receive \
                     nothing, got {keys:?}"
                );
                tokio::time::sleep(PROBE).await;
            }
        }
    }
}

/// Poll `DescribeClusterV2` until the cluster reports `ACTIVE`, and answer that answer.
async fn until_active(router: &MultiServiceRouter, case: &str, cluster_arn: &str) -> Value {
    let uri = format!("/api/v2/clusters/{}", label(cluster_arn));
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, described) = kafka(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeClusterV2 must succeed, got {status} {described}"
        );
        if described.pointer("/clusterInfo/state").and_then(Value::as_str) == Some("ACTIVE") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the cluster never reached ACTIVE, last read {described}"
        );
        tokio::time::sleep(PROBE).await;
    }
}

/// Poll the group `name` until it holds at least one event, and answer them.
async fn until_events(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let events = events_in(router, name).await;
        if !events.is_empty() {
            return events;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the log group {name:?} the cluster declares received no broker log \
             event"
        );
        tokio::time::sleep(PROBE).await;
    }
}

/// Poll the bucket `bucket` until it holds at least one object, and answer their keys.
async fn until_objects(router: &MultiServiceRouter, case: &str, bucket: &str) -> Vec<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let keys = keys_in(router, bucket).await;
        if !keys.is_empty() {
            return keys;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the bucket {bucket:?} the cluster declares received no broker log \
             object"
        );
        tokio::time::sleep(PROBE).await;
    }
}

/// Assert the group `name` holds no event and no stream, looked at repeatedly so a delivery
/// that merely arrives later would still be caught.
async fn assert_stays_empty(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    what: &str,
) {
    for _ in 0..QUIET_LOOKS {
        let events = events_in(router, name).await;
        assert!(
            events.is_empty(),
            "case {case:?}: {what} ({name:?}) must receive no event, got {events:?}"
        );
        let streams = streams_in(router, name).await;
        assert!(
            streams.is_empty(),
            "case {case:?}: {what} ({name:?}) must receive no stream, got {streams:?}"
        );
        tokio::time::sleep(PROBE).await;
    }
}

/// RULE, the whole chain a client of broker logs walks: a provisioned cluster created with
/// `LoggingInfo.BrokerLogs` is accepted, reaches `ACTIVE`, publishes a PLAINTEXT bootstrap
/// address that really accepts a connection and answers the Apache Kafka protocol on it,
/// and the lines its brokers wrote reach the group and the bucket that `BrokerLogs` named.
///
/// The address is dialled exactly as published, and the `ApiVersions` request is framed by
/// hand, so what is asserted is what the broker answered on the wire.
#[tokio::test]
async fn the_published_plaintext_endpoint_answers_and_its_brokers_logs_are_delivered() {
    let case = "the published plaintext endpoint";
    let router = emulator();
    let group = "msk-broker-logs-served-wired";
    let unwired = "msk-broker-logs-served-unwired";
    let bucket = "msk-broker-logs-served";
    let prefix = "served/msk";
    given_log_group(&router, case, group).await;
    given_log_group(&router, case, unwired).await;
    given_bucket(&router, case, bucket).await;

    let (status, answered) = kafka(
        &router,
        "POST",
        "/v1/clusters",
        Some(json!({
            "clusterName": "msk-broker-logs-served",
            "kafkaVersion": KAFKA_VERSION,
            "numberOfBrokerNodes": SUBNETS.len(),
            "brokerNodeGroupInfo": {
                "instanceType": INSTANCE_TYPE,
                "clientSubnets": SUBNETS,
            },
            "encryptionInfo": {
                "encryptionInTransit": { "clientBroker": "PLAINTEXT", "inCluster": true },
            },
            "clientAuthentication": { "unauthenticated": { "enabled": true } },
            "loggingInfo": {
                "brokerLogs": {
                    "cloudWatchLogs": { "enabled": true, "logGroup": group },
                    "s3": { "enabled": true, "bucket": bucket, "prefix": prefix },
                    "firehose": { "enabled": false },
                },
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCluster must be accepted, got {status} {answered}"
    );
    let cluster_arn = answered
        .get("clusterArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the create must report a clusterArn, got {answered}"))
        .to_string();
    until_active(&router, case, &cluster_arn).await;

    // The address the control plane publishes for the access method this cluster offers in
    // the clear, taken exactly as it was written.
    let (status, published) = kafka(
        &router,
        "GET",
        &format!("/v1/clusters/{}/bootstrap-brokers", label(&cluster_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetBootstrapBrokers must succeed, got {status} {published}"
    );
    let bootstrap = published
        .get("bootstrapBrokerString")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a PLAINTEXT cluster must publish a bootstrap string, got {published}")
        })
        .to_string();
    let pairs: Vec<&str> = bootstrap.split(',').map(str::trim).collect();
    assert_eq!(
        pairs.len(),
        SUBNETS.len(),
        "case {case:?}: expected one host:port pair per broker node, got {bootstrap:?}"
    );

    // Every published pair accepts a connection and answers the protocol, advertising the
    // APIs a client needs to administer topics and produce to them.
    for pair in &pairs {
        let negotiated = api_versions(case, pair).await;
        assert_eq!(
            negotiated.error_code, 0,
            "case {case:?}: {pair} must answer ApiVersions with no error, got {}",
            negotiated.error_code
        );
        for required in REQUIRED_API_KEYS {
            assert!(
                negotiated.api_keys.contains(&required),
                "case {case:?}: {pair} must advertise api key {required}, got {:?}",
                negotiated.api_keys
            );
        }
    }

    // And the lines the brokers wrote reach both destinations the cluster named, while the
    // group it was never wired to stays empty.
    let events = until_events(&router, case, group).await;
    for (_, message) in &events {
        assert!(
            is_broker_log_line(message),
            "case {case:?}: every delivered event must be an Apache Kafka broker log line, got \
             {message:?}"
        );
    }
    let keys = until_objects(&router, case, bucket).await;
    let body = object(&router, case, bucket, &keys[0]).await;
    assert_eq!(
        body.get(..2),
        Some(GZIP_MAGIC.as_slice()),
        "case {case:?}: a delivered log file is a gzip stream, got the first bytes {:?}",
        body.get(..2)
    );
    assert!(
        keys[0].starts_with(&format!(
            "{prefix}/{DELIVERED_LOGS_SEGMENT}/{ACCOUNT}/{BROKER_LOGS_SEGMENT}/{REGION}/"
        )),
        "case {case:?}: a delivered object sits under the declared prefix, got {:?}",
        keys[0]
    );
    assert_stays_empty(&router, case, unwired, "the unwired group").await;
}
