// SPDX-License-Identifier: CC0-1.0

//! A Kinesis shard ITERATOR, end to end over the wire: `CreateStream` -> `ListShards` ->
//! `GetShardIterator`.
//!
//! RULE: `GetShardIterator` answers an opaque iterator that names the position its
//! `ShardIteratorType` asks for inside one shard of the stream its request addresses, for
//! every type the model declares; it refuses a request whose members its own type does not
//! admit, or that names no position at all, with `InvalidArgumentException`, and one naming a
//! stream this account and region do not hold, a shard the stream does not hold, or a stream
//! that is not `ACTIVE` yet with `ResourceNotFoundException`. Handing an iterator out changes
//! nothing a later caller sees: two calls for the same position answer iterators that name
//! the same shard and position.
//!
//! The recorded flow this was derived from takes ONE iterator of the first shard of a live
//! stream (`get_shard_iterator(StreamName=…, ShardId=…, ShardIteratorType="TRIM_HORIZON")`)
//! and is one row of the tables below. The rows differ along what the rule generalises over:
//! which iterator type names the position, which shard of the map it is taken in, which
//! identifier the stream is resolved by, which member the type reads its position from, and
//! the status the stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_id, shard_not_found_message, shard_not_in_stream_message, ShardIteratorToken,
    ShardPosition,
    ACTIVATION_DELAY, SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER,
    SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER, SHARD_ITERATOR_TYPE_AT_TIMESTAMP,
    SHARD_ITERATOR_TYPE_LATEST, SHARD_ITERATOR_TYPE_TRIM_HORIZON,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The modelled maximum length of a `ShardIterator`
/// (`smithy_query(query="operation",operation="GetShardIterator")` -> `ShardIterator`
/// len 1..512).
const MAX_SHARD_ITERATOR_LEN: usize = 512;

/// Every `ShardIteratorType` the model declares
/// (`smithy_query(query="shape",shape="ShardIteratorType")`).
const SHARD_ITERATOR_TYPES: &[&str] = &[
    SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER,
    SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
    SHARD_ITERATOR_TYPE_AT_TIMESTAMP,
    SHARD_ITERATOR_TYPE_LATEST,
    SHARD_ITERATOR_TYPE_TRIM_HORIZON,
];

/// How long a case waits before treating a freshly created stream as ready: past the instant
/// the create armed the `CREATING` -> `ACTIVE` transition for.
fn past_activation() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(400)
}

/// A router carrying exactly this service, over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kinesis/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body {response}"
        );
        response
    }

    /// Call `operation` expecting a client refusal, answering its error code and message.
    async fn refused(&self, case: &str, operation: &str, body: &Value) -> (String, String) {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected {operation} to answer 400, got {status} with body {response}"
        );
        (error_code(&response), error_message(&response))
    }

    /// A provisioned stream of `shard_count` shards, past the transition the create armed.
    async fn active_stream(&self, case: &str, name: &str, shard_count: i64) -> String {
        self.ok(
            case,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": shard_count }),
        )
        .await;
        tokio::time::sleep(past_activation()).await;
        let summary = self
            .ok(case, "DescribeStreamSummary", &json!({"StreamName": name}))
            .await;
        summary["StreamDescriptionSummary"]["StreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary reported no StreamARN"))
            .to_string()
    }

    /// The `SequenceNumberRange.StartingSequenceNumber` the shard map publishes for
    /// `shard`, which is the only sequence number a reader can name before anything is
    /// written.
    async fn starting_sequence_number(&self, case: &str, stream: &str, shard: &str) -> String {
        let listed = self
            .ok(case, "ListShards", &json!({"StreamName": stream}))
            .await;
        listed["Shards"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: ListShards reported no Shards"))
            .iter()
            .find(|entry| entry["ShardId"].as_str() == Some(shard))
            .map(|entry| entry["SequenceNumberRange"]["StartingSequenceNumber"].clone())
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| panic!("case {case}: the shard map carries no {shard}"))
    }

    /// The `ShardIterator` a successful `GetShardIterator` answered.
    async fn iterator(&self, case: &str, body: &Value) -> String {
        let answer = self.ok(case, "GetShardIterator", body).await;
        let iterator = answer["ShardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ShardIterator, got {answer}"))
            .to_string();
        assert!(
            !iterator.is_empty() && iterator.len() <= MAX_SHARD_ITERATOR_LEN,
            "case {case}: a ShardIterator is 1..{MAX_SHARD_ITERATOR_LEN} characters, got {} \
             characters",
            iterator.len()
        );
        iterator
    }
}

/// The `__type` an awsJson1_1 error body carries, reduced to the error's shape name.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an awsJson1_1 error body carries, under either casing the protocols use.
fn error_message(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Epoch milliseconds, the unit `GetShardIteratorInput$Timestamp` carries ("A time stamp is
/// the Unix epoch date with precision in milliseconds").
///
/// Rounded UP to the next whole second, for two reasons: an awsJson `Timestamp` travels as
/// epoch seconds with a fractional part, so a whole second survives that round trip exactly;
/// and the instant is then past the stream's trim horizon, which for a stream nothing has
/// retained data in yet is the instant it was created — a timestamp before that horizon is
/// answered at the horizon instead, which is the subject of its own case below.
fn now_millis() -> u128 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_millis();
    millis + (1_000 - millis % 1_000)
}

/// Every iterator type, in every shard of the map, resolved by either identifier: the
/// iterator a client is handed names the shard it asked for and the position its type
/// denotes.
#[tokio::test]
async fn every_iterator_type_names_the_requested_position_in_the_requested_shard() {
    let emulator = Emulator::new();
    let stream = "iterators-every-type";
    let arn = emulator.active_stream("every type", stream, 3).await;
    let first_sequence_number = emulator
        .starting_sequence_number("every type", stream, &shard_id(0))
        .await;
    let at_timestamp = now_millis();

    // (case, the iterator type, the member its position comes from)
    let mut seen: Vec<String> = Vec::new();
    for shard_index in 0..3u32 {
        let shard = shard_id(shard_index);
        let sequence_number = emulator
            .starting_sequence_number("every type", stream, &shard)
            .await;
        for iterator_type in SHARD_ITERATOR_TYPES {
            // Rows differ along which identifier addresses the stream, so both forms are
            // exercised across the table.
            let identifier = if shard_index % 2 == 0 {
                json!({"StreamName": stream})
            } else {
                json!({"StreamARN": arn})
            };
            let mut body = identifier;
            body["ShardId"] = json!(shard);
            body["ShardIteratorType"] = json!(iterator_type);
            match *iterator_type {
                SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER
                | SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER => {
                    body["StartingSequenceNumber"] = json!(sequence_number);
                }
                SHARD_ITERATOR_TYPE_AT_TIMESTAMP => {
                    // A timestamp travels as epoch SECONDS on the wire for a `Timestamp`
                    // member of an awsJson protocol, with millisecond precision.
                    body["Timestamp"] = json!(at_timestamp as f64 / 1_000.0);
                }
                _ => {}
            }
            let case = format!("{iterator_type} in {shard}");
            let iterator = emulator.iterator(&case, &body).await;
            let decoded = ShardIteratorToken::decode(&iterator).unwrap_or_else(|| {
                panic!("case {case}: the iterator this service issued must decode")
            });
            assert_eq!(
                decoded.shard_id, shard,
                "case {case}: the iterator must name the shard the request asked for, got \
                 {decoded:?}"
            );
            assert_eq!(
                decoded.stream_arn, arn,
                "case {case}: the iterator must name the addressed stream, got {decoded:?}"
            );
            let expected = match *iterator_type {
                SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER => {
                    ShardPosition::AtSequenceNumber(sequence_number.clone())
                }
                SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER => {
                    ShardPosition::AfterSequenceNumber(sequence_number.clone())
                }
                SHARD_ITERATOR_TYPE_AT_TIMESTAMP => ShardPosition::AtTimestamp(at_timestamp),
                SHARD_ITERATOR_TYPE_LATEST => ShardPosition::Latest,
                _ => ShardPosition::TrimHorizon,
            };
            assert_eq!(
                decoded.position, expected,
                "case {case}: the iterator must name the position its type denotes, got \
                 {decoded:?}"
            );
            assert!(
                !decoded.is_expired(SystemTime::now()),
                "case {case}: a freshly issued iterator is not expired, got {decoded:?}"
            );
            seen.push(iterator);
        }
    }
    assert_eq!(
        seen.len(),
        3 * SHARD_ITERATOR_TYPES.len(),
        "every shard of the map answers every iterator type, got {} iterators",
        seen.len()
    );

    // Handing out an iterator writes nothing a later caller can see: the same request twice
    // names the same shard and the same position.
    let repeat_body = json!({
        "StreamName": stream,
        "ShardId": shard_id(0),
        "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
        "StartingSequenceNumber": first_sequence_number,
    });
    let first = emulator.iterator("repeat", &repeat_body).await;
    let second = emulator.iterator("repeat", &repeat_body).await;
    let (first, second) = (
        ShardIteratorToken::decode(&first).expect("the first iterator decodes"),
        ShardIteratorToken::decode(&second).expect("the second iterator decodes"),
    );
    assert_eq!(
        (first.shard_id, first.position.clone()),
        (second.shard_id, second.position),
        "case repeat: two calls for one position name the same shard and position"
    );
}

/// A timestamp older than the stream's trim horizon is answered with the oldest untrimmed
/// position: "If the time stamp is older than the current trim horizon, the iterator returned
/// is for the oldest untrimmed data record (TRIM_HORIZON)."
#[tokio::test]
async fn a_timestamp_before_the_trim_horizon_is_answered_at_the_trim_horizon() {
    let emulator = Emulator::new();
    let stream = "iterators-horizon";
    emulator.active_stream("horizon", stream, 1).await;

    // (case, the timestamp the request names, the position it is answered at)
    let now = now_millis();
    let day_millis = 24 * 60 * 60 * 1_000u128;
    let cases = [
        (
            "two days before the horizon",
            now.saturating_sub(2 * day_millis),
            ShardPosition::TrimHorizon,
        ),
        ("inside the retention window", now, ShardPosition::AtTimestamp(now)),
    ];
    for (case, at, expected) in cases {
        let iterator = emulator
            .iterator(
                case,
                &json!({
                    "StreamName": stream,
                    "ShardId": shard_id(0),
                    "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_TIMESTAMP,
                    "Timestamp": at as f64 / 1_000.0,
                }),
            )
            .await;
        let decoded =
            ShardIteratorToken::decode(&iterator).expect("the iterator this service issued decodes");
        assert_eq!(
            decoded.position, expected,
            "case {case}: expected the position {expected:?}, got {decoded:?}"
        );
    }
}

/// Every refusal the rule names, each with the modelled error code it is refused under.
#[tokio::test]
async fn a_request_that_names_no_position_or_no_shard_is_refused() {
    let emulator = Emulator::new();
    let stream = "iterators-refusals";
    emulator.active_stream("refusals", stream, 2).await;
    let sequence_number = emulator
        .starting_sequence_number("refusals", stream, &shard_id(0))
        .await;
    let missing_shard = shard_id(9);

    // (case, the request, the modelled error code it is refused under)
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a stream this account and region do not hold",
            json!({
                "StreamName": "iterators-no-such-stream",
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
            "ResourceNotFoundException",
        ),
        (
            "a shard the stream does not hold",
            json!({
                "StreamName": stream,
                "ShardId": missing_shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
            "ResourceNotFoundException",
        ),
        (
            "neither StreamName nor StreamARN",
            json!({
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
            "InvalidArgumentException",
        ),
        (
            "AT_SEQUENCE_NUMBER without a StartingSequenceNumber",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
            }),
            "InvalidArgumentException",
        ),
        (
            "AFTER_SEQUENCE_NUMBER without a StartingSequenceNumber",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER,
            }),
            "InvalidArgumentException",
        ),
        (
            "AT_TIMESTAMP without a Timestamp",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_TIMESTAMP,
            }),
            "InvalidArgumentException",
        ),
        (
            "TRIM_HORIZON with a StartingSequenceNumber",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                "StartingSequenceNumber": sequence_number,
            }),
            "InvalidArgumentException",
        ),
        (
            "LATEST with a Timestamp",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_LATEST,
                "Timestamp": now_millis() as f64 / 1_000.0,
            }),
            "InvalidArgumentException",
        ),
        (
            "a sequence number of no shard of the stream",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
                "StartingSequenceNumber": "1",
            }),
            "InvalidArgumentException",
        ),
        (
            "a dry run of an otherwise valid request",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                "DryRun": true,
            }),
            "DryRunOperationException",
        ),
    ];
    for (case, body, expected) in cases {
        let (code, message) = emulator.refused(case, "GetShardIterator", &body).await;
        assert_eq!(
            code, expected,
            "case {case}: expected {expected}, got {code} with message {message:?}"
        );
        assert!(
            !message.is_empty(),
            "case {case}: a refusal carries a message, got none"
        );
    }

    // The two not-found refusals are both phrased at the SHARD this read asked for, each in
    // the sentence its own condition earns: a shard of a stream this account does not hold
    // does not exist at all, while a shard the HELD stream's map does not carry could not be
    // found in it.
    let (_, missing_stream_message) = emulator
        .refused(
            "the missing stream is named",
            "GetShardIterator",
            &json!({
                "StreamName": "iterators-no-such-stream",
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await;
    assert_eq!(
        missing_stream_message,
        shard_not_found_message(&shard_id(0), "iterators-no-such-stream"),
        "a shard of a missing stream is refused with the service's shard-not-found sentence"
    );
    let (_, missing_shard_message) = emulator
        .refused(
            "the missing shard is named",
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": missing_shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await;
    assert_eq!(
        missing_shard_message,
        shard_not_in_stream_message(&missing_shard, stream),
        "a shard the held stream does not carry is refused with the service's sentence for it"
    );
}

/// A stream whose creation has not fallen due yet hands out no iterator: the shard is not
/// readable, which is the modelled not-found refusal.
#[tokio::test]
async fn a_stream_that_is_not_active_yet_hands_out_no_iterator() {
    let emulator = Emulator::new();
    let stream = "iterators-creating";
    emulator
        .ok(
            "creating",
            "CreateStream",
            &json!({"StreamName": stream, "ShardCount": 1}),
        )
        .await;
    let (code, message) = emulator
        .refused(
            "creating",
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case creating: expected ResourceNotFoundException, got {code} with message {message:?}"
    );

    // Once the transition the create armed has fallen due, the same request is served.
    tokio::time::sleep(past_activation()).await;
    let iterator = emulator
        .iterator(
            "active",
            &json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await;
    assert!(
        ShardIteratorToken::decode(&iterator).is_some(),
        "case active: the iterator an active stream answers must decode, got {iterator}"
    );
}
