// SPDX-License-Identifier: CC0-1.0

//! The FAN-OUT half of a subscription filter, end to end over the wire with the Kinesis
//! emulator mounted beside this service on the same engine, so a delivery is observed
//! where a consumer observes it: as records a shard serves.
//!
//! RULE: every log event a `PutLogEvents` batch stores is offered to each subscription
//! filter of its log group; the filter's own `filterPattern` decides which of those events
//! it carries ("you can subscribe to a real-time stream of log events ingested through
//! PutLogEvents and have them delivered to a specific destination"), and the ones it
//! carries are handed to the resource the filter's `destinationArn` resolves to as a
//! gzip-framed JSON envelope ("When log events are sent to the receiving service, they are
//! Base64 encoded and compressed with the GZIP format") naming the owner account, the log
//! group, the log stream, the filter and the events, each event carrying the message
//! exactly as it was ingested. The log group's `kmsKeyId` is a property of the group's own
//! stored data, not of any destination, so it changes nothing about what is delivered; and
//! `distribution` decides only the key the envelope is placed under ("By default, log data
//! is grouped by log stream, but the grouping can be set to random for a more even
//! distribution").
//!
//! The recorded flow this was derived from writes ONE batch to a CMK-encrypted group whose
//! filter matches everything and reads the records off a one-shard Kinesis stream; that is
//! one row of the table below. The rows differ along what the rule generalises over:
//! whether the group names a KMS key at all, the FORM of the filter pattern (match
//! everything, an unstructured term, a JSON condition), which of the batch's events the
//! pattern selects, how many events it selects, and the `distribution` the filter is
//! configured with.
//!
//! The second and third tests are the rest of the same rule: a `destinationArn` that names
//! a LOGICAL destination is resolved to the physical target that destination names, and a
//! batch that owes no delivery — no filter, a filter nothing matches, or a destination that has
//! gone away since the filter was configured — is still ingested and still answered.
//!
//! Every case drives the real wire path (`POST /` with `X-Amz-Target:
//! <Service>.<Operation>` through `MultiServiceRouter::dispatch`), so what is asserted here
//! is what a client sees.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_id, ACTIVATION_DELAY, DELETION_DELAY, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{base_ms, REGION};

/// `X-Amz-Target` prefixes of the services this suite drives.
pub(crate) const LOGS_TARGET_PREFIX: &str = "Logs_20140328";
const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";
/// CloudWatch's prefix and the signing name its credential scope carries (`sigv4Name:
/// monitoring`), for the suites that read what a delivery published as metric data.
pub(crate) const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
pub(crate) const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The awsJson wire each service speaks: CloudWatch Logs and Kinesis on 1.1, CloudWatch on
/// 1.0.
const JSON_1_1: &str = "application/x-amz-json-1.1";
pub(crate) const JSON_1_0: &str = "application/x-amz-json-1.0";

/// The `messageType` of an envelope that carries ingested log events.
pub(crate) const DATA_MESSAGE: &str = "DATA_MESSAGE";

/// The `messageType` of the health-check envelope a `PutSubscriptionFilter` writes to the
/// destination it configures.
pub(crate) const CONTROL_MESSAGE: &str = "CONTROL_MESSAGE";

/// The digits a delivered log event's `id` carries.
const EVENT_ID_DIGITS: usize = 56;

/// The `distribution` values the model declares (`Distribution`, one of: ByLogStream |
/// Random).
pub(crate) const BY_LOG_STREAM: &str = "ByLogStream";
const RANDOM: &str = "Random";

/// How long a case waits before treating a freshly created Kinesis stream as ready: past
/// the instant its create armed the `CREATING` -> `ACTIVE` transition for.
pub(crate) fn past_activation() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(400)
}

/// The status a Kinesis stream reports once it can take a delivery, and how long a case gives
/// it to report that ([`Emulator::await_active_stream`]) and how often it asks. The deadline
/// is generous: it is not a limit any case here tests, only room for a loaded host.
const STREAM_ACTIVE: &str = "ACTIVE";
const STREAM_ACTIVE_DEADLINE: Duration = Duration::from_secs(60);
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a case waits before treating a deleted Kinesis stream as gone: past the instant
/// its delete armed the removal for, after which the name is free again.
pub(crate) fn past_deletion() -> Duration {
    DELETION_DELAY + Duration::from_millis(400)
}

/// CloudWatch Logs, Kinesis and CloudWatch on ONE shared engine, reached over the wire —
/// the same assembly `core/router` performs, with three services mounted. The in-process
/// callback a delivery travels over is the router's own, so a record that is not written by
/// the Kinesis service cannot be read back here, and a datapoint that is not published into
/// CloudWatch's own metric store cannot be read back through CloudWatch.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signing is not the subject here, so requests are unauthenticated; the credential
        // scope is still carried, because it is what names the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One awsJson1_1 call, signed for [`REGION`] under `service`'s own scope.
    pub(crate) async fn call(
        &self,
        service: &str,
        prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        self.call_on_wire(service, prefix, JSON_1_1, operation, body)
            .await
    }

    /// [`Emulator::call`] on the awsJson wire `content_type` names, for a service that does
    /// not speak 1.1.
    pub(crate) async fn call_on_wire(
        &self,
        service: &str,
        prefix: &str,
        content_type: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", content_type)
            .header("x-amz-target", format!("{prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One CloudWatch Logs call that must succeed.
    pub(crate) async fn logs(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call("logs", LOGS_TARGET_PREFIX, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One CloudWatch call that must succeed — CloudWatch speaks awsJson1_0 and signs under
    /// its own name, which is the only difference from any other call here.
    pub(crate) async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call_on_wire(
                CLOUDWATCH_SIGNING_NAME,
                CLOUDWATCH_TARGET_PREFIX,
                JSON_1_0,
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// One Kinesis call that must succeed.
    pub(crate) async fn kinesis(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call("kinesis", KINESIS_TARGET_PREFIX, operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: {operation} {body} must succeed, got {status} {parsed}"
        );
        parsed
    }

    /// The DATA records the one-shard stream `stream` holds, from its oldest retained
    /// record: each record's partition key and the envelope its gzip-framed data decodes to.
    ///
    /// A `PutSubscriptionFilter` writes a health check to the same stream
    /// ([`Emulator::health_checks`]); that record carries no ingested log event, so it is
    /// not one of the deliveries a suite about forwarding reads.
    pub(crate) async fn delivered(&self, case: &str, stream: &str) -> Vec<(String, Value)> {
        self.envelopes(case, stream, DATA_MESSAGE).await
    }

    /// The HEALTH-CHECK records the stream holds: what `PutSubscriptionFilter` wrote to
    /// validate the destination it was handed.
    pub(crate) async fn health_checks(&self, case: &str, stream: &str) -> Vec<(String, Value)> {
        self.envelopes(case, stream, CONTROL_MESSAGE).await
    }

    /// Every envelope on the stream whose `messageType` is `message_type`.
    async fn envelopes(
        &self,
        case: &str,
        stream: &str,
        message_type: &str,
    ) -> Vec<(String, Value)> {
        self.shard_records(case, stream)
            .await
            .into_iter()
            .map(|(key, framed)| (key, gunzip_json(case, &framed)))
            .filter(|(_, envelope)| envelope["messageType"] == json!(message_type))
            .collect()
    }

    /// The same data records, as the BYTES each one's `Data` carries — what a consumer reads
    /// before it decompresses anything, which is where the framing itself is observable.
    pub(crate) async fn delivered_frames(
        &self,
        case: &str,
        stream: &str,
    ) -> Vec<(String, Vec<u8>)> {
        self.shard_records(case, stream)
            .await
            .into_iter()
            .filter(|(_, framed)| {
                gunzip_json(case, framed)["messageType"] == json!(DATA_MESSAGE)
            })
            .collect()
    }

    /// EVERY record the one-shard stream holds, from its oldest retained one: each record's
    /// partition key and the bytes its `Data` carries, whatever the envelope says it is.
    pub(crate) async fn shard_records(&self, case: &str, stream: &str) -> Vec<(String, Vec<u8>)> {
        self.sequenced_shard_records(case, stream)
            .await
            .into_iter()
            .map(|(_, key, framed)| (key, framed))
            .collect()
    }

    /// The same records, each with the SEQUENCE NUMBER the shard gave it — the shard's own
    /// ordering, which is where the order of two records is observable at all.
    pub(crate) async fn sequenced_shard_records(
        &self,
        case: &str,
        stream: &str,
    ) -> Vec<(String, String, Vec<u8>)> {
        let iterator = self
            .kinesis(
                case,
                "GetShardIterator",
                &json!({
                    "StreamName": stream,
                    "ShardId": shard_id(0),
                    "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                }),
            )
            .await["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: GetShardIterator reported no ShardIterator"))
            .to_string();
        let answer = self
            .kinesis(
                case,
                "GetRecords",
                &json!({"ShardIterator": iterator, "Limit": 100}),
            )
            .await;
        answer["Records"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|record| {
                let encoded = record["Data"].as_str().unwrap_or_else(|| {
                    panic!("case {case}: a record carries Data, got {record}")
                });
                let framed = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                    panic!("case {case}: a record's Data is base64, got {error} for {record}")
                });
                let key = record["PartitionKey"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a record carries a PartitionKey, got {record}")
                    })
                    .to_string();
                let sequence_number = record["SequenceNumber"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: a record carries a SequenceNumber, got {record}")
                    })
                    .to_string();
                (sequence_number, key, framed)
            })
            .collect()
    }

    /// A Kinesis data stream of one shard, created but not yet waited for.
    pub(crate) async fn create_stream(&self, case: &str, stream: &str) {
        self.kinesis(
            case,
            "CreateStream",
            &json!({"StreamName": stream, "ShardCount": 1}),
        )
        .await;
    }

    /// A Kinesis data stream of one shard, READY: created, then waited for until it reports
    /// itself usable.
    pub(crate) async fn usable_stream(&self, case: &str, stream: &str) {
        self.create_stream(case, stream).await;
        self.await_active_stream(case, stream).await;
    }

    /// Wait until `stream` reports itself usable — polled for, because the status it leaves
    /// `CREATING` for is owed by the clock, and the wait ends as soon as the stream says so.
    ///
    /// The deadline is generous: it is not a limit any case tests, only room for a loaded
    /// host.
    pub(crate) async fn await_active_stream(&self, case: &str, stream: &str) {
        let deadline = std::time::Instant::now() + STREAM_ACTIVE_DEADLINE;
        loop {
            let summary = self
                .kinesis(case, "DescribeStreamSummary", &json!({"StreamName": stream}))
                .await;
            if summary["StreamDescriptionSummary"]["StreamStatus"] == json!(STREAM_ACTIVE) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "case {case}: expected the stream {stream} to report {STREAM_ACTIVE}, got \
                 {summary}"
            );
            tokio::time::sleep(STREAM_POLL_INTERVAL).await;
        }
    }

    /// A logical destination over the Kinesis stream `stream`, USABLE: `PutDestination`
    /// creates it and `PutDestinationPolicy` attaches the access policy that authorizes a
    /// subscription filter against it, because the two calls are one flow — "By default,
    /// PutDestination does not set any access policy with the destination [...] To enable
    /// this, the destination owner must call PutDestinationPolicy after PutDestination"
    /// (`PutDestination`). Answers the destination's own ARN, which is what a filter names.
    pub(crate) async fn logical_destination(
        &self,
        case: &str,
        name: &str,
        stream: &str,
    ) -> String {
        let created = self
            .logs(
                case,
                "PutDestination",
                &json!({
                    "destinationName": name,
                    "targetArn": stream_arn(stream),
                    "roleArn": role_arn(),
                }),
            )
            .await;
        let arn = created["destination"]["arn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: PutDestination reports the destination's arn, got {created}")
            })
            .to_string();
        let access_policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": "AllowThisAccountToSubscribe",
                "Effect": "Allow",
                "Principal": {
                    "AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
                },
                "Action": "logs:PutSubscriptionFilter",
                "Resource": arn,
            }],
        })
        .to_string();
        self.logs(
            case,
            "PutDestinationPolicy",
            &json!({"destinationName": name, "accessPolicy": access_policy}),
        )
        .await;
        arn
    }
}

/// The BYTES a delivered record's gzip frame carries. A frame that is not gzip fails the
/// case here rather than later: the framing IS part of the rule.
pub(crate) fn gunzip(case: &str, framed: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    ::flate2::read::GzDecoder::new(framed)
        .read_to_end(&mut body)
        .unwrap_or_else(|error| {
            panic!("case {case}: a delivered record is a gzip frame, got {error}")
        });
    body
}

/// The JSON a delivered record's gzip frame carries. A frame that does not hold JSON fails
/// the case here rather than later, for the same reason.
fn gunzip_json(case: &str, framed: &[u8]) -> Value {
    let body = String::from_utf8(gunzip(case, framed))
        .unwrap_or_else(|error| panic!("case {case}: the frame holds UTF-8, got {error}"));
    serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("case {case}: the frame holds JSON, got {error} for {body}"))
}

/// The ARN of the Kinesis data stream `stream`, in the Region every call here is signed
/// for and the account every ARN this emulator mints carries.
pub(crate) fn stream_arn(stream: &str) -> String {
    engine::emulator_arn("kinesis", REGION, &format!("stream/{stream}"))
}

/// The role CloudWatch Logs assumes to deliver to a streaming destination —
/// `PutSubscriptionFilter` requires one for every destination except a Lambda function.
pub(crate) fn role_arn() -> String {
    engine::emulator_arn("iam", "", "role/CWLtoKinesisRole")
}

/// A KMS key ARN, the form `CreateLogGroup$kmsKeyId` documents ("The Amazon Resource Name
/// (ARN) of the KMS key to use when encrypting log data").
fn cmk_arn() -> String {
    engine::emulator_arn(
        "kms",
        REGION,
        "key/12345678-1234-1234-1234-123456789012",
    )
}

/// One row of the fan-out table.
struct Row {
    /// What the case is called in every assertion it makes.
    case: &'static str,
    /// Whether the log group names a KMS key, which must change nothing about the
    /// delivered envelope.
    encrypted: bool,
    /// The `filterPattern` the subscription filter is configured with.
    pattern: &'static str,
    /// The `distribution` the filter is configured with.
    distribution: &'static str,
    /// The messages the batch carries, in the chronological order a batch is written in.
    messages: &'static [&'static str],
    /// The messages the filter's pattern selects, in the order they were written.
    delivered: &'static [&'static str],
}

/// The table: one row per form of the rule, spanning the group's encryption, the pattern
/// form, how much of the batch the pattern selects and the distribution.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "unencrypted group, match-everything pattern",
            encrypted: false,
            pattern: "",
            distribution: BY_LOG_STREAM,
            messages: &["alpha", "ERROR boom"],
            delivered: &["alpha", "ERROR boom"],
        },
        Row {
            // The recorded case: a CMK-encrypted group delivers the SAME plaintext
            // envelope as the unencrypted one above.
            case: "cmk-encrypted group, match-everything pattern",
            encrypted: true,
            pattern: "",
            distribution: BY_LOG_STREAM,
            messages: &["alpha", "ERROR boom"],
            delivered: &["alpha", "ERROR boom"],
        },
        Row {
            case: "cmk-encrypted group, unstructured term pattern",
            encrypted: true,
            pattern: "ERROR",
            distribution: BY_LOG_STREAM,
            messages: &["alpha", "ERROR boom", "ERROR again"],
            delivered: &["ERROR boom", "ERROR again"],
        },
        Row {
            case: "unencrypted group, JSON condition pattern",
            encrypted: false,
            pattern: "{$.level = \"WARN\"}",
            distribution: BY_LOG_STREAM,
            messages: &[
                "{\"level\":\"INFO\",\"msg\":\"up\"}",
                "{\"level\":\"WARN\",\"msg\":\"slow\"}",
            ],
            delivered: &["{\"level\":\"WARN\",\"msg\":\"slow\"}"],
        },
        Row {
            case: "cmk-encrypted group, pattern selecting nothing",
            encrypted: true,
            pattern: "NOTHING_MATCHES_THIS",
            distribution: BY_LOG_STREAM,
            messages: &["alpha", "ERROR boom"],
            delivered: &[],
        },
        Row {
            case: "unencrypted group, randomly distributed",
            encrypted: false,
            pattern: "",
            distribution: RANDOM,
            messages: &["alpha"],
            delivered: &["alpha"],
        },
    ]
}

/// Assert one envelope against the row that produced it.
pub(crate) fn assert_envelope(
    case: &str,
    envelope: &Value,
    group: &str,
    stream: &str,
    filter: &str,
    messages: &[&str],
    timestamps: &[i64],
) {
    let member = |name: &str| -> String {
        envelope[name]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the envelope carries {name}, got {envelope}"))
            .to_string()
    };
    assert_eq!(
        member("messageType"),
        DATA_MESSAGE,
        "case {case}: an envelope of ingested events is a {DATA_MESSAGE}, got {envelope}"
    );
    assert_eq!(
        member("owner"),
        engine::emulator_account_id(),
        "case {case}: the envelope names the owning account, got {envelope}"
    );
    assert_eq!(
        member("logGroup"),
        group,
        "case {case}: the envelope names the log group, got {envelope}"
    );
    assert_eq!(
        member("logStream"),
        stream,
        "case {case}: the envelope names the log stream, got {envelope}"
    );
    assert_eq!(
        envelope["subscriptionFilters"],
        json!([filter]),
        "case {case}: the envelope names the filter that selected the events, got {envelope}"
    );

    let events = envelope["logEvents"].as_array().cloned().unwrap_or_default();
    let delivered: Vec<String> = events
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every delivered event carries a message, got {event}")
                })
                .to_string()
        })
        .collect();
    assert_eq!(
        delivered,
        messages.iter().map(|m| (*m).to_string()).collect::<Vec<_>>(),
        "case {case}: expected the events the pattern selects, got {delivered:?}"
    );
    let stamps: Vec<i64> = events
        .iter()
        .map(|event| {
            event["timestamp"].as_i64().unwrap_or_else(|| {
                panic!("case {case}: every delivered event carries a timestamp, got {event}")
            })
        })
        .collect();
    assert_eq!(
        stamps, timestamps,
        "case {case}: a delivered event keeps the timestamp it was ingested with, got {stamps:?}"
    );
    let mut ids: Vec<String> = events
        .iter()
        .map(|event| {
            let id = event["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: every delivered event carries an id, got {event}")
                })
                .to_string();
            assert_eq!(
                id.len(),
                EVENT_ID_DIGITS,
                "case {case}: an event id is {EVENT_ID_DIGITS} characters, got {id}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_digit()),
                "case {case}: an event id is decimal, got {id}"
            );
            id
        })
        .collect();
    let minted = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "case {case}: two delivered events never share an id, got {ids:?}"
    );
}

/// The table: what a matching log event's delivery looks like at its filter's Kinesis
/// destination, over every form of the rule.
#[tokio::test]
async fn a_matching_event_is_delivered_as_a_plaintext_gzip_envelope() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every destination stream is created first and the one activation span waited out
    // once, so the table does not pay it per row.
    for (index, row) in rows.iter().enumerate() {
        emulator
            .create_stream(row.case, &format!("fanout-{index}"))
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let stream = format!("fanout-{index}");
        let group = format!("group-{index}");
        let log_stream = format!("log-stream-{index}");
        let filter = format!("filter-{index}");

        let mut create = json!({"logGroupName": group});
        if row.encrypted {
            create["kmsKeyId"] = json!(cmk_arn());
        }
        emulator.logs(case, "CreateLogGroup", &create).await;
        emulator
            .logs(
                case,
                "CreateLogStream",
                &json!({"logGroupName": group, "logStreamName": log_stream}),
            )
            .await;
        emulator
            .logs(
                case,
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": filter,
                    "filterPattern": row.pattern,
                    "destinationArn": stream_arn(&stream),
                    "roleArn": role_arn(),
                    "distribution": row.distribution,
                }),
            )
            .await;

        let base = base_ms();
        let timestamps: Vec<i64> = (0..row.messages.len())
            .map(|offset| base + i64::try_from(offset).expect("the offset fits an i64"))
            .collect();
        let events: Vec<Value> = row
            .messages
            .iter()
            .zip(&timestamps)
            .map(|(message, timestamp)| json!({"timestamp": timestamp, "message": message}))
            .collect();
        emulator
            .logs(
                case,
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": log_stream,
                    "logEvents": events,
                }),
            )
            .await;

        // The ingestion itself is unaffected by the fan-out: every event the batch carried
        // is readable back through the log stream.
        let read = emulator
            .logs(
                case,
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": log_stream}),
            )
            .await;
        let stored: Vec<String> = read["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| event["message"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            stored,
            row.messages
                .iter()
                .map(|m| (*m).to_string())
                .collect::<Vec<_>>(),
            "case {case}: the batch is still ingested whole, got {stored:?}"
        );

        let records = emulator.delivered(case, &stream).await;
        if row.delivered.is_empty() {
            assert!(
                records.is_empty(),
                "case {case}: a pattern that selects nothing delivers nothing, got {records:?}"
            );
            continue;
        }
        assert_eq!(
            records.len(),
            1,
            "case {case}: one batch owes its filter one envelope, got {records:?}"
        );
        let (key, envelope) = &records[0];
        let delivered_timestamps: Vec<i64> = row
            .messages
            .iter()
            .zip(&timestamps)
            .filter(|(message, _)| row.delivered.contains(message))
            .map(|(_, timestamp)| *timestamp)
            .collect();
        assert_envelope(
            case,
            envelope,
            &group,
            &log_stream,
            &filter,
            row.delivered,
            &delivered_timestamps,
        );
        if row.distribution == BY_LOG_STREAM {
            assert_eq!(
                key, &log_stream,
                "case {case}: log data grouped by log stream is keyed on the log stream, \
                 got {key}"
            );
        } else {
            assert_ne!(
                key, &log_stream,
                "case {case}: a randomly distributed filter does not group by log stream, \
                 got {key}"
            );
        }
    }
}

/// A `destinationArn` naming a LOGICAL destination is resolved to the physical target that
/// destination names, and the envelope arrives there.
#[tokio::test]
async fn a_logical_destination_delivers_to_the_physical_target_it_names() {
    let case = "logical destination";
    let emulator = Emulator::new();
    let stream = "logical-target";
    emulator.create_stream(case, stream).await;
    tokio::time::sleep(past_activation()).await;

    let destination_arn = emulator
        .logical_destination(case, "to-the-stream", stream)
        .await;

    let group = "logical-group";
    let log_stream = "logical-log-stream";
    emulator
        .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": log_stream}),
        )
        .await;
    // A logical destination carries the role it delivers through, so the filter names none.
    emulator
        .logs(
            case,
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "through-the-destination",
                "filterPattern": "",
                "destinationArn": destination_arn,
            }),
        )
        .await;

    let timestamp = base_ms();
    emulator
        .logs(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": log_stream,
                "logEvents": [{"timestamp": timestamp, "message": "indirect"}],
            }),
        )
        .await;

    let records = emulator.delivered(case, stream).await;
    assert_eq!(
        records.len(),
        1,
        "case {case}: the envelope reaches the destination's target, got {records:?}"
    );
    assert_envelope(
        case,
        &records[0].1,
        group,
        log_stream,
        "through-the-destination",
        &["indirect"],
        &[timestamp],
    );
}

/// An ingestion that owes no delivery is still answered: a group with no filter at all, and
/// a filter whose destination has GONE AWAY since it was configured.
///
/// A destination that does not exist cannot be configured at all — `PutSubscriptionFilter`
/// validates it and refuses one its own service holds nothing under — so the undeliverable
/// case is reached the one way it can arise: the stream is there when the filter is put, and
/// deleted afterwards.
#[tokio::test]
async fn an_undeliverable_or_unsubscribed_batch_is_still_ingested() {
    let emulator = Emulator::new();
    let cases: [(&str, Option<&str>); 2] = [
        ("no subscription filter", None),
        ("destination deleted after the filter was configured", Some("deleted-after-put")),
    ];
    for (index, (case, destination)) in cases.iter().enumerate() {
        let group = format!("quiet-group-{index}");
        let log_stream = format!("quiet-log-stream-{index}");
        emulator
            .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        emulator
            .logs(
                case,
                "CreateLogStream",
                &json!({"logGroupName": group, "logStreamName": log_stream}),
            )
            .await;
        if let Some(stream) = destination {
            emulator.usable_stream(case, stream).await;
            emulator
                .logs(
                    case,
                    "PutSubscriptionFilter",
                    &json!({
                        "logGroupName": group,
                        "filterName": "to-nowhere",
                        "filterPattern": "",
                        "destinationArn": stream_arn(stream),
                        "roleArn": role_arn(),
                    }),
                )
                .await;
            emulator
                .kinesis(
                    case,
                    "DeleteStream",
                    &json!({"StreamName": stream, "EnforceConsumerDeletion": true}),
                )
                .await;
        }
        let timestamp = base_ms();
        let answer = emulator
            .logs(
                case,
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": log_stream,
                    "logEvents": [{"timestamp": timestamp, "message": "kept"}],
                }),
            )
            .await;
        assert!(
            answer["nextSequenceToken"].is_string(),
            "case {case}: PutLogEvents is always accepted and mints a token, got {answer}"
        );
        let read = emulator
            .logs(
                case,
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": log_stream}),
            )
            .await;
        let stored: Vec<String> = read["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|event| event["message"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            stored,
            vec!["kept".to_string()],
            "case {case}: the event is stored whatever its filter's destination answered, \
             got {stored:?}"
        );
    }
}
