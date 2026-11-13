// SPDX-License-Identifier: CC0-1.0

//! The enhanced fan-out consumer, end to end over the wire: `RegisterStreamConsumer` ->
//! `DescribeStreamConsumer` / `DescribeStreamSummary` / `DeleteStream`.
//!
//! RULE: `RegisterStreamConsumer` PERSISTS a consumer on the stream its `StreamARN`
//! addresses, under the `ConsumerName` the request carried, and answers the `Consumer`
//! shape that stored record projects onto: an ARN Kinesis generates from the stream's ARN,
//! the consumer's name and the registration instant ("consumer ARNs contain the creation
//! timestamp", `Consumer$ConsumerARN`), the `CREATING` status a consumer starts in — "a
//! consumer can't read data while in the CREATING or DELETING states"
//! (`Consumer$ConsumerStatus`) — and the instant it was registered. It reports `CREATING`
//! until the transition the registration armed falls due and `ACTIVE` from then on.
//! `DescribeStreamConsumer` projects that STORED record onto `ConsumerDescription`,
//! resolving it by `ConsumerARN`, by `StreamARN` and `ConsumerName`, or by all three as
//! long as they name one consumer, so what a registration wrote is what a read reports.
//! A name the stream already carries a consumer under is refused with the modelled
//! `ResourceInUseException` and leaves the stored consumer untouched, while the same name
//! on another stream is another consumer; a registration past the consumers one stream may
//! carry, or past the consumers that may be in `CREATING` at once, is
//! `LimitExceededException`; a request naming no consumer is `InvalidArgumentException` and
//! one naming a consumer this account and region do not hold is
//! `ResourceNotFoundException`. `DescribeStreamSummary`'s `ConsumerCount` counts the
//! consumers registered with the stream, and a `DeleteStream` that does not set
//! `EnforceConsumerDeletion` is refused while the stream carries any.
//!
//! The recorded flow this was derived from registers ONE consumer with ONE stream and is
//! one row of the tables below. The rows differ along what the rule generalises over: the
//! form and length of `ConsumerName` inside its `@pattern`/`@length`, how many tags the
//! registration carries (none, some, the documented ceiling), the capacity mode of the
//! stream, how many consumers the stream already carries, the identifier a read resolves
//! by, and the region the request is signed for.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    consumer_already_exists_message, consumer_not_found_message, epoch_seconds,
    stream_has_consumers_message, ACTIVATION_DELAY, CONSUMER_ACTIVATION_DELAY, CONSUMER_ACTIVE,
    CONSUMER_CREATING, CONSUMER_IDENTIFIER_REQUIRED_MESSAGE, DELETION_DELAY,
    MAX_CONSUMERS_CREATING, MAX_CONSUMERS_PER_STREAM, MAX_CONSUMER_TAGS, STREAM_MODE_ON_DEMAND,
    STREAM_MODE_PROVISIONED,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of this service (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "Kinesis_20131202";

/// The `@length` maximum of `ConsumerName`, in characters
/// (`smithy_query(query="operation",operation="RegisterStreamConsumer")` -> `ConsumerName`
/// len 1..128).
const CONSUMER_NAME_MAX_LENGTH: usize = 128;

/// How long past a transition's due instant a case waits before reading the settled state.
const SETTLE_SLACK: Duration = Duration::from_millis(400);

/// A router carrying exactly this service, over one shared engine: the same assembly
/// `core/router` performs, with one service mounted.
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

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesis/\
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

    /// [`Self::call_in`] in the default test region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation` in `region`, asserting a 200 and answering the body.
    async fn ok_in(&self, case: &str, region: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} with body \
             {response}"
        );
        response
    }

    /// [`Self::ok_in`] in the default test region.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        self.ok_in(case, REGION, operation, body).await
    }

    /// Create an ACTIVE stream named `stream_name` in `region` and answer its ARN: every
    /// consumer case needs a stream to register against, and registering is only served
    /// against a stream whose create has settled.
    async fn active_stream_in(
        &self,
        case: &str,
        region: &str,
        stream_name: &str,
        stream_mode: &str,
    ) -> String {
        let mut body = json!({
            "StreamName": stream_name,
            "StreamModeDetails": { "StreamMode": stream_mode },
        });
        if stream_mode == STREAM_MODE_PROVISIONED {
            body["ShardCount"] = json!(1);
        }
        self.ok_in(case, region, "CreateStream", &body).await;
        let summary = self
            .await_active_stream(case, region, stream_name)
            .await
            .get("StreamDescriptionSummary")
            .cloned()
            .unwrap_or(Value::Null);
        summary
            .get("StreamARN")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// [`Self::active_stream_in`] in the default test region, with a provisioned stream.
    async fn active_stream(&self, case: &str, stream_name: &str) -> String {
        self.active_stream_in(case, REGION, stream_name, STREAM_MODE_PROVISIONED)
            .await
    }

    /// Wait out the transition the create armed and answer the stream's summary.
    async fn await_active_stream(&self, case: &str, region: &str, stream_name: &str) -> Value {
        // The transition is owed at a stored instant, so waiting past it is all it takes.
        tokio::time::sleep(ACTIVATION_DELAY + SETTLE_SLACK).await;
        self.ok_in(
            case,
            region,
            "DescribeStreamSummary",
            &json!({ "StreamName": stream_name }),
        )
        .await
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

/// A string member of `value`, or the empty string when the body carries none.
fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `ConsumerName` of `length` characters inside the modelled `^[a-zA-Z0-9_.-]+$`.
fn name_of_length(length: usize) -> String {
    std::iter::repeat_n('c', length).collect()
}

/// `count` tags, as a `TagMap` body.
fn tags_of(count: usize) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        tags.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(tags)
}

/// Assert `consumer` is the consumer `consumer_name` names on `stream_arn`: the ARN is the
/// one Kinesis generates for it, the status is `expected_status`, and the creation
/// timestamp sits in the window the call was served in.
fn assert_consumer(
    case: &str,
    consumer: &Value,
    stream_arn: &str,
    consumer_name: &str,
    expected_status: &str,
    registered_from: SystemTime,
) {
    assert_eq!(
        text(consumer, "ConsumerName"),
        consumer_name,
        "case {case}: the consumer must carry the name the registration named, got \
         {consumer:?}"
    );
    assert_eq!(
        text(consumer, "ConsumerStatus"),
        expected_status,
        "case {case}: expected status {expected_status}, got {consumer:?}"
    );

    // "When you register a consumer, Kinesis Data Streams generates an ARN for it...
    // consumer ARNs contain the creation timestamp" (`Consumer$ConsumerARN`), so the ARN is
    // the stream's, the consumer's name and the instant it was registered.
    let consumer_arn = text(consumer, "ConsumerARN");
    let prefix = format!("{stream_arn}/consumer/{consumer_name}:");
    let suffix = consumer_arn.strip_prefix(&prefix);
    assert!(
        suffix.is_some(),
        "case {case}: expected a ConsumerARN starting {prefix:?}, got {consumer_arn:?}"
    );
    let registered_at = suffix
        .unwrap_or_default()
        .parse::<u64>()
        .unwrap_or_default();
    let window = epoch_seconds(registered_from)..=epoch_seconds(SystemTime::now());
    assert!(
        window.contains(&registered_at),
        "case {case}: the ConsumerARN's creation timestamp must be when the consumer was \
         registered, expected one of {window:?}, got {registered_at} in {consumer_arn:?}"
    );

    let created = consumer
        .get("ConsumerCreationTimestamp")
        .and_then(Value::as_f64)
        .unwrap_or_default()
        .trunc();
    let created_window = window.clone();
    assert!(
        created_window.contains(&(created as u64)),
        "case {case}: the consumer's creation timestamp must be when it was registered, \
         expected one of {created_window:?}, got {created} in {consumer:?}"
    );
}

/// One `RegisterStreamConsumer` request: the stream it registers against and the consumer
/// it names.
struct RegisterCase {
    /// What an assertion calls this row.
    label: &'static str,
    stream_name: &'static str,
    /// The capacity mode of the stream the consumer is registered with: a consumer is
    /// registered with a stream of either mode.
    stream_mode: &'static str,
    consumer_name: String,
    /// `Tags`, absent on a row that names none.
    tags: Option<Value>,
}

impl RegisterCase {
    /// The `RegisterStreamConsumerInput` body this row sends against `stream_arn`.
    fn body(&self, stream_arn: &str) -> Value {
        let mut body = json!({ "StreamARN": stream_arn, "ConsumerName": self.consumer_name });
        if let Some(tags) = &self.tags {
            body["Tags"] = tags.clone();
        }
        body
    }
}

/// The table the register/read rule is checked over. The rows differ along the form and
/// length of `ConsumerName` (both published `@length` bounds and a punctuated name inside
/// the pattern), how many tags the registration carries (none, a handful, the documented
/// ceiling of 50), and the capacity mode of the stream registered against.
fn register_cases() -> Vec<RegisterCase> {
    vec![
        RegisterCase {
            label: "shortest-name-no-tags",
            stream_name: "sim2real-consumers-a",
            stream_mode: STREAM_MODE_PROVISIONED,
            // The modelled `@length` minimum of `ConsumerName`.
            consumer_name: name_of_length(1),
            tags: None,
        },
        RegisterCase {
            label: "punctuated-name-two-tags",
            stream_name: "sim2real-consumers-b",
            stream_mode: STREAM_MODE_PROVISIONED,
            consumer_name: "sim.2real_consumer-a.1".to_string(),
            tags: Some(json!({ "Team": "iot", "Env": "test" })),
        },
        RegisterCase {
            label: "longest-name-tag-ceiling-on-demand-stream",
            stream_name: "sim2real-consumers-c",
            stream_mode: STREAM_MODE_ON_DEMAND,
            consumer_name: name_of_length(CONSUMER_NAME_MAX_LENGTH),
            // "A set of up to 50 key-value pairs" (`RegisterStreamConsumerInput$Tags`).
            tags: Some(tags_of(MAX_CONSUMER_TAGS)),
        },
    ]
}

#[tokio::test]
async fn a_registered_consumer_is_reported_by_arn_and_by_stream_and_name() {
    let emulator = Emulator::new();

    for case in register_cases() {
        let label = case.label;
        let stream_arn = emulator
            .active_stream_in(label, REGION, case.stream_name, case.stream_mode)
            .await;

        let registered_from = SystemTime::now();
        let registered = emulator
            .ok(
                label,
                "RegisterStreamConsumer",
                &case.body(&stream_arn),
            )
            .await;
        let consumer = registered
            .get("Consumer")
            .cloned()
            .unwrap_or(Value::Null);
        assert_consumer(
            label,
            &consumer,
            &stream_arn,
            &case.consumer_name,
            // "A consumer can't read data while in the CREATING or DELETING states"
            // (`Consumer$ConsumerStatus`): a consumer starts in CREATING.
            CONSUMER_CREATING,
            registered_from,
        );
        let consumer_arn = text(&consumer, "ConsumerARN");

        // Every way the read resolves one consumer: its ARN, the stream and the name, and
        // all three at once ("You may also provide all three parameters, as long as they
        // don't conflict with each other", `DescribeStreamConsumer`).
        let reads = vec![
            ("by-consumer-arn", json!({ "ConsumerARN": consumer_arn })),
            (
                "by-stream-and-name",
                json!({ "StreamARN": stream_arn, "ConsumerName": case.consumer_name }),
            ),
            (
                "by-all-three",
                json!({
                    "StreamARN": stream_arn,
                    "ConsumerName": case.consumer_name,
                    "ConsumerARN": consumer_arn,
                }),
            ),
        ];
        for (how, body) in &reads {
            let read = format!("{label}/{how}");
            let described = emulator
                .ok(&read, "DescribeStreamConsumer", body)
                .await
                .get("ConsumerDescription")
                .cloned()
                .unwrap_or(Value::Null);
            assert_consumer(
                &read,
                &described,
                &stream_arn,
                &case.consumer_name,
                CONSUMER_CREATING,
                registered_from,
            );
            assert_eq!(
                text(&described, "StreamARN"),
                stream_arn,
                "case {read}: the description must name the stream the consumer is \
                 registered with, got {described:?}"
            );
        }

        // The transition is owed at a stored instant, so waiting past it is all it takes.
        tokio::time::sleep(CONSUMER_ACTIVATION_DELAY + SETTLE_SLACK).await;
        let settled = emulator
            .ok(
                label,
                "DescribeStreamConsumer",
                &json!({ "ConsumerARN": consumer_arn }),
            )
            .await
            .get("ConsumerDescription")
            .cloned()
            .unwrap_or(Value::Null);
        assert_consumer(
            label,
            &settled,
            &stream_arn,
            &case.consumer_name,
            CONSUMER_ACTIVE,
            registered_from,
        );
        assert_eq!(
            text(&settled, "ConsumerARN"),
            consumer_arn,
            "case {label}: the consumer's ARN must not change when it turns \
             {CONSUMER_ACTIVE}, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn a_consumer_name_is_unique_inside_one_stream_only() {
    let emulator = Emulator::new();
    let case = "duplicate-name";
    let shared_name = "sim2real-shared-consumer";
    let other_region = "eu-west-1";

    // Two streams in the default region, and one more under the same name in another
    // region: a consumer name is unique inside ONE stream.
    let first = emulator.active_stream(case, "sim2real-consumers-dup-1").await;
    let second = emulator.active_stream(case, "sim2real-consumers-dup-2").await;
    let regional = emulator
        .active_stream_in(
            case,
            other_region,
            "sim2real-consumers-dup-1",
            STREAM_MODE_PROVISIONED,
        )
        .await;

    let first_arn = text(
        &emulator
            .ok(
                case,
                "RegisterStreamConsumer",
                &json!({ "StreamARN": first, "ConsumerName": shared_name }),
            )
            .await
            .get("Consumer")
            .cloned()
            .unwrap_or(Value::Null),
        "ConsumerARN",
    );

    // Every row re-registers the SAME name; only the stream it is registered with differs,
    // and with it whether the name is already taken.
    let rows: Vec<(&str, &str, &str, &str, bool)> = vec![
        (
            "same-stream",
            REGION,
            "sim2real-consumers-dup-1",
            first.as_str(),
            false,
        ),
        (
            "other-stream",
            REGION,
            "sim2real-consumers-dup-2",
            second.as_str(),
            true,
        ),
        (
            "same-stream-name-other-region",
            other_region,
            "sim2real-consumers-dup-1",
            regional.as_str(),
            true,
        ),
    ];
    for (row, region, stream_name, stream_arn, served) in &rows {
        let label = format!("{case}/{row}");
        let (status, response) = emulator
            .call_in(
                region,
                "RegisterStreamConsumer",
                &json!({ "StreamARN": stream_arn, "ConsumerName": shared_name }),
            )
            .await;
        if *served {
            // "However, consumer names don't have to be unique across data streams"
            // (`RegisterStreamConsumerInput$ConsumerName`).
            assert_eq!(
                status, 200,
                "case {label}: the same name on another stream is another consumer, got \
                 {status} with body {response}"
            );
            let consumer_arn = text(
                &response.get("Consumer").cloned().unwrap_or(Value::Null),
                "ConsumerARN",
            );
            assert_ne!(
                consumer_arn, first_arn,
                "case {label}: another stream's consumer must carry its own ARN, got \
                 {consumer_arn}"
            );
        } else {
            // "For a given Kinesis data stream, each consumer must have a unique name"
            // (`RegisterStreamConsumerInput$ConsumerName`).
            assert_eq!(
                status, 400,
                "case {label}: a name the stream already carries must be refused, got \
                 {status} with body {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceInUseException",
                "case {label}: expected ResourceInUseException, got body {response}"
            );
            let message = error_message(&response);
            assert_eq!(
                message,
                consumer_already_exists_message(shared_name, stream_name),
                "case {label}: the refusal must name the consumer, the stream and the \
                 account at fault, got {message}"
            );
        }
    }

    // The consumer the duplicate was refused against is left exactly as it was.
    let described = emulator
        .ok(
            case,
            "DescribeStreamConsumer",
            &json!({ "ConsumerARN": first_arn }),
        )
        .await
        .get("ConsumerDescription")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        text(&described, "ConsumerARN"),
        first_arn,
        "case {case}: the stored consumer must survive the refused duplicate, got \
         {described:?}"
    );
}

#[tokio::test]
async fn the_stream_summary_counts_the_consumers_registered_with_it() {
    let emulator = Emulator::new();
    let case = "consumer-count";
    let stream_name = "sim2real-consumers-count";
    let stream_arn = emulator.active_stream(case, stream_name).await;

    // One row per consumer registered, so the count is asserted after each one: it tracks
    // the consumers the stream carries rather than any one number.
    for registered in 0..MAX_CONSUMERS_CREATING {
        let summary = emulator
            .ok(
                case,
                "DescribeStreamSummary",
                &json!({ "StreamARN": stream_arn }),
            )
            .await
            .get("StreamDescriptionSummary")
            .cloned()
            .unwrap_or(Value::Null);
        let counted = summary
            .get("ConsumerCount")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        assert_eq!(
            counted as usize, registered,
            "case {case}: expected {registered} consumers counted, got {counted} in \
             {summary:?}"
        );
        emulator
            .ok(
                case,
                "RegisterStreamConsumer",
                &json!({
                    "StreamARN": stream_arn,
                    "ConsumerName": format!("consumer-{registered}"),
                }),
            )
            .await;
    }
}

#[tokio::test]
async fn a_registration_past_a_documented_ceiling_is_refused() {
    let emulator = Emulator::new();
    let case = "consumer-ceilings";
    let stream_arn = emulator.active_stream(case, "sim2real-consumers-full").await;

    // "You cannot have more than 5 consumers in a CREATING status at the same time.
    // Registering a 6th consumer while there are 5 in a CREATING status results in a
    // LimitExceededException" (`RegisterStreamConsumer`).
    for index in 0..MAX_CONSUMERS_CREATING {
        emulator
            .ok(
                case,
                "RegisterStreamConsumer",
                &json!({ "StreamARN": stream_arn, "ConsumerName": format!("creating-{index}") }),
            )
            .await;
    }
    let (status, response) = emulator
        .call(
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": "creating-overflow" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}/creating: a registration while {MAX_CONSUMERS_CREATING} consumers are \
         {CONSUMER_CREATING} must be refused, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {case}/creating: expected LimitExceededException, got body {response}"
    );

    // Past the CREATING window the same request is served, so the ceiling on how many
    // consumers one stream carries at all is what refuses the next ones: "you can register
    // up to 20 consumers per stream to use Enhanced Fan-out" (`RegisterStreamConsumer`).
    let mut registered = MAX_CONSUMERS_CREATING;
    while registered < MAX_CONSUMERS_PER_STREAM {
        tokio::time::sleep(CONSUMER_ACTIVATION_DELAY + SETTLE_SLACK).await;
        let batch = (MAX_CONSUMERS_PER_STREAM - registered).min(MAX_CONSUMERS_CREATING);
        for index in registered..registered + batch {
            emulator
                .ok(
                    case,
                    "RegisterStreamConsumer",
                    &json!({
                        "StreamARN": stream_arn,
                        "ConsumerName": format!("consumer-{index}"),
                    }),
                )
                .await;
        }
        registered += batch;
    }
    tokio::time::sleep(CONSUMER_ACTIVATION_DELAY + SETTLE_SLACK).await;
    let (status, response) = emulator
        .call(
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": "consumer-overflow" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}/per-stream: the {}th consumer of a stream must be refused, got {status} \
         with body {response}",
        MAX_CONSUMERS_PER_STREAM + 1
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {case}/per-stream: expected LimitExceededException, got body {response}"
    );

    // The stream carries exactly the consumers that were registered: the refusals stored
    // nothing.
    let summary = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamARN": stream_arn }),
        )
        .await
        .get("StreamDescriptionSummary")
        .cloned()
        .unwrap_or(Value::Null);
    let counted = summary
        .get("ConsumerCount")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    assert_eq!(
        counted as usize,
        MAX_CONSUMERS_PER_STREAM,
        "case {case}: expected {MAX_CONSUMERS_PER_STREAM} consumers counted, got {counted} \
         in {summary:?}"
    );
}

#[tokio::test]
async fn a_registration_or_a_read_naming_no_such_resource_is_refused() {
    let emulator = Emulator::new();
    let case = "unknown-resources";
    let stream_name = "sim2real-consumers-known";
    let stream_arn = emulator.active_stream(case, stream_name).await;
    let consumer_name = "sim2real-known-consumer";
    let consumer_arn = text(
        &emulator
            .ok(
                case,
                "RegisterStreamConsumer",
                &json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name }),
            )
            .await
            .get("Consumer")
            .cloned()
            .unwrap_or(Value::Null),
        "ConsumerARN",
    );

    // An ARN of a stream this account and region do not hold, inside the modelled
    // `StreamARN` pattern.
    let unknown_stream = format!(
        "arn:aws:kinesis:{REGION}:{}:stream/sim2real-consumers-absent",
        engine::EMULATOR_ACCOUNT_ID
    );
    // Each row: the operation, its body, and the modelled error code it must answer.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "register-unknown-stream",
            "RegisterStreamConsumer",
            json!({ "StreamARN": unknown_stream, "ConsumerName": consumer_name }),
            "ResourceNotFoundException",
        ),
        (
            "describe-unknown-stream",
            "DescribeStreamConsumer",
            json!({ "StreamARN": unknown_stream, "ConsumerName": consumer_name }),
            "ResourceNotFoundException",
        ),
        (
            "describe-unknown-consumer-name",
            "DescribeStreamConsumer",
            json!({ "StreamARN": stream_arn, "ConsumerName": "sim2real-absent-consumer" }),
            "ResourceNotFoundException",
        ),
        (
            "describe-unknown-consumer-arn",
            "DescribeStreamConsumer",
            json!({ "ConsumerARN": format!("{stream_arn}/consumer/{consumer_name}:1") }),
            "ResourceNotFoundException",
        ),
        (
            "describe-no-consumer-identifier",
            "DescribeStreamConsumer",
            json!({}),
            "InvalidArgumentException",
        ),
        (
            "describe-stream-without-a-name",
            "DescribeStreamConsumer",
            json!({ "StreamARN": stream_arn }),
            "InvalidArgumentException",
        ),
        (
            "describe-conflicting-identifiers",
            "DescribeStreamConsumer",
            json!({ "ConsumerARN": consumer_arn, "ConsumerName": "sim2real-other-consumer" }),
            "InvalidArgumentException",
        ),
    ];
    for (row, operation, body, expected) in &rows {
        let label = format!("{case}/{row}");
        let (status, response) = emulator.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected {operation} to refuse the request, got {status} with \
             body {response}"
        );
        assert_eq!(
            &error_code(&response),
            expected,
            "case {label}: expected {expected}, got body {response}"
        );
    }

    // The refusals are worded by the rule, not by the row: a read naming no consumer says
    // which members name one, and a read naming an absent consumer names it.
    let (_, unspecified) = emulator.call("DescribeStreamConsumer", &json!({})).await;
    assert_eq!(
        error_message(&unspecified),
        CONSUMER_IDENTIFIER_REQUIRED_MESSAGE,
        "case {case}: a read naming no consumer must say which members name one, got body \
         {unspecified}"
    );
    let absent = "sim2real-absent-consumer";
    let (_, not_found) = emulator
        .call(
            "DescribeStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": absent }),
        )
        .await;
    assert_eq!(
        error_message(&not_found),
        consumer_not_found_message(absent, &stream_arn),
        "case {case}: a read naming an absent consumer must name it, got body {not_found}"
    );

    // A read that resolves by `ConsumerARN` alone still names the consumer and the stream
    // that ARN carries: the name the consumer was registered under, the creation timestamp
    // that tells two consumers of one name apart ("consumer ARNs contain the creation
    // timestamp", `Consumer$ConsumerARN`), and the stream the ARN extends — rather than the
    // ARN echoed back whole.
    let absent_arn = format!("{stream_arn}/consumer/{absent}:1");
    let (_, arn_not_found) = emulator
        .call(
            "DescribeStreamConsumer",
            &json!({ "ConsumerARN": absent_arn }),
        )
        .await;
    let message = error_message(&arn_not_found);
    assert!(
        message.contains(absent)
            && message.contains("with creation timestamp 1")
            && message.contains(stream_name)
            && !message.contains(absent_arn.as_str()),
        "case {case}: a read by ConsumerARN alone must name the consumer the ARN carries, its \
         creation timestamp and the stream it extends, got {message}"
    );
}

#[tokio::test]
async fn a_consumer_is_registered_only_with_a_stream_that_is_ready() {
    let emulator = Emulator::new();
    let case = "stream-not-ready";
    let stream_name = "sim2real-consumers-creating";
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": stream_name, "ShardCount": 1 }),
        )
        .await;
    let stream_arn = format!(
        "arn:aws:kinesis:{REGION}:{}:stream/{stream_name}",
        engine::EMULATOR_ACCOUNT_ID
    );

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`).
    let (status, response) = emulator
        .call(
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": "sim2real-early-consumer" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: a registration against a stream that is not ready must be refused, \
         got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {case}: expected ResourceInUseException, got body {response}"
    );

    // Once the stream's own transition falls due the same request is served.
    tokio::time::sleep(ACTIVATION_DELAY + SETTLE_SLACK).await;
    let consumer = emulator
        .ok(
            case,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": "sim2real-early-consumer" }),
        )
        .await
        .get("Consumer")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        text(&consumer, "ConsumerName"),
        "sim2real-early-consumer",
        "case {case}: the registration must be served once the stream is ready, got \
         {consumer:?}"
    );
}

#[tokio::test]
async fn deleting_a_stream_with_consumers_needs_the_enforce_flag() {
    let emulator = Emulator::new();
    let case = "enforce-consumer-deletion";
    let stream_name = "sim2real-consumers-deleted";
    let stream_arn = emulator.active_stream(case, stream_name).await;
    let consumer_name = "sim2real-deleted-consumer";
    emulator
        .ok(
            case,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name }),
        )
        .await;

    // Each row: the `EnforceConsumerDeletion` the delete carries, and whether the stream
    // with a registered consumer may go. "If this parameter is unset (null) or if you set
    // it to false, and the stream has registered consumers, the call to DeleteStream fails
    // with a ResourceInUseException" (`DeleteStreamInput$EnforceConsumerDeletion`).
    let refused: Vec<(&str, Value)> = vec![
        ("unset", json!({ "StreamARN": stream_arn })),
        (
            "false",
            json!({ "StreamARN": stream_arn, "EnforceConsumerDeletion": false }),
        ),
    ];
    for (row, body) in &refused {
        let label = format!("{case}/{row}");
        let (status, response) = emulator.call("DeleteStream", body).await;
        assert_eq!(
            status, 400,
            "case {label}: a stream with a registered consumer must not be deleted, got \
             {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceInUseException",
            "case {label}: expected ResourceInUseException, got body {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message,
            stream_has_consumers_message(stream_name),
            "case {label}: the refusal must name the stream standing in the way and the \
             member that serves the call anyway, got {message}"
        );
    }

    // The consumer is still there: the refusals changed nothing.
    emulator
        .ok(
            case,
            "DescribeStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name }),
        )
        .await;

    emulator
        .ok(
            case,
            "DeleteStream",
            &json!({ "StreamARN": stream_arn, "EnforceConsumerDeletion": true }),
        )
        .await;
    // The removal is owed at a stored instant, so waiting past it is all it takes; the
    // consumers registered with the stream go with it.
    tokio::time::sleep(DELETION_DELAY + SETTLE_SLACK).await;
    let (status, response) = emulator
        .call(
            "DescribeStreamConsumer",
            &json!({ "StreamARN": stream_arn, "ConsumerName": consumer_name }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: the consumers of a deleted stream must be gone with it, got {status} \
         with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case {case}: expected ResourceNotFoundException, got body {response}"
    );
}
