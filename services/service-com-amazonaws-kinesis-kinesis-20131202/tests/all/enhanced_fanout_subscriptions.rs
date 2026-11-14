// SPDX-License-Identifier: CC0-1.0

//! Enhanced fan-out subscriptions, end to end over the wire: `CreateStream` ->
//! `RegisterStreamConsumer` -> `PutRecord` -> `SubscribeToShard`.
//!
//! RULE: `SubscribeToShard` is SERVED from this service's own state — HTTP 200 whose body is
//! an AWS event stream opening with the frame a client reads the output shape out of
//! (`:event-type` `initial-response`), then one `SubscribeToShardEvent` carrying the records
//! the shard retains from the position the request named, the `ContinuationSequenceNumber` a
//! consumer reads on from and how far behind the tip of the shard it is — and every request it
//! will not serve is refused with one of the errors the MODEL declares for it, each a client
//! error at HTTP 400 (`AccessDeniedException`, `DryRunOperationException`,
//! `InvalidArgumentException`, `LimitExceededException`, `ResourceInUseException`,
//! `ResourceNotFoundException`), never with a status or an error code the model does not
//! declare. The records it pushes are the records the pull reader (`GetShardIterator` ->
//! `GetRecords`) serves for the same position, because one rule decides where a reader of a
//! shard starts and what one read of it delivers.
//!
//! The recorded flows this was derived from subscribe with a `ConsumerARN` naming a stream
//! that does not exist, and with one naming a consumer that was never registered; both are
//! rows of the tables below. The other rows differ along what the rule generalises over: which
//! of the five `ShardIteratorType` positions the request names and which member supplies it,
//! how many records the position admits, which resource of the subscription is missing, which
//! member of the starting position its own type does not admit, whether the shard is open or
//! closed by a reshard, and whether the consumer already holds a subscription to the shard.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! headers, frames and modelled errors asserted here are the ones a client sees.

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    dry_run_succeeded_message, CONSUMER_ACTIVE,
};

use crate::stream_tags::{
    error_code, error_message, poll_until, poll_until_active, stream_arn, Emulator, REGION,
};

/// The media type an event-stream body carries.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The `:event-type` of the frame an event-stream response opens with, which carries the
/// output's non-stream members (source:
/// `service_common::event_stream::INITIAL_RESPONSE_EVENT`).
const INITIAL_RESPONSE: &str = "initial-response";

/// The `SubscribeToShardEventStream` member a data event is framed as (source:
/// `smithy_query(query="shape",shape="SubscribeToShardEventStream")`).
const DATA_EVENT: &str = "SubscribeToShardEvent";

/// The shard id a one-shard stream's only shard is laid down under.
const FIRST_SHARD: &str = "shardId-000000000000";

/// A shard id no stream of these cases holds.
const ABSENT_SHARD: &str = "shardId-000000000099";

/// The middle of the hash key space, inside the modelled `HashKey` pattern
/// (`^(0|([1-9]\d{0,38}))$`): what the split case cuts a shard at.
const MIDDLE_HASH_KEY: &str = "170141183460469231731687303715884105728";

/// A provisioned stream of `shard_count` shards, past the transition its create armed.
async fn active_stream(emulator: &Emulator, case: &str, name: &str, shard_count: i64) {
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": shard_count }),
        )
        .await;
    poll_until_active(emulator, case, name).await;
}

/// A consumer registered with `stream` under `name`, past the transition its registration
/// armed — "a consumer can't read data while in the CREATING or DELETING states"
/// (`Consumer$ConsumerStatus`) — answering its `ConsumerARN`.
async fn active_consumer(emulator: &Emulator, case: &str, stream: &str, name: &str) -> String {
    let registered = emulator
        .ok(
            case,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn(REGION, stream), "ConsumerName": name }),
        )
        .await;
    let arn = registered["Consumer"]["ConsumerARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a registration answers a ConsumerARN"))
        .to_string();
    poll_until(
        case,
        &format!("the consumer {name} reports {CONSUMER_ACTIVE}"),
        || async {
            let (status, response) = emulator
                .call("DescribeStreamConsumer", &json!({ "ConsumerARN": arn }))
                .await;
            status == 200
                && response["ConsumerDescription"]["ConsumerStatus"].as_str()
                    == Some(CONSUMER_ACTIVE)
        },
    )
    .await;
    arn
}

/// Write one record to `stream`, answering the sequence number it was stored under.
async fn put(emulator: &Emulator, case: &str, stream: &str, payload: &str) -> String {
    emulator
        .ok(
            case,
            "PutRecord",
            &json!({
                "StreamName": stream,
                "Data": aws_smithy_types::base64::encode(payload),
                "PartitionKey": "one-key",
            }),
        )
        .await["SequenceNumber"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: PutRecord answers a SequenceNumber"))
        .to_string()
}

/// One decoded event-stream frame: whether it is an event or an in-band exception, the union
/// member it is framed as, and its payload.
struct Frame {
    message_type: String,
    event_type: String,
    payload: Value,
}

/// Decode an event-stream body, asserting the frame headers an SDK decoder requires.
fn decode_frames(case: &str, body: &[u8]) -> Vec<Frame> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case}: the body must decode as event-stream frames: {error}")
        });
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        let message_type = header(":message-type")
            .unwrap_or_else(|| panic!("case {case}: a frame named no :message-type"));
        let event_type = header(":event-type")
            .or_else(|| header(":exception-type"))
            .unwrap_or_else(|| panic!("case {case}: a frame named no :event-type"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!("case {case}: the {event_type} payload must be JSON: {error}")
        });
        frames.push(Frame {
            message_type,
            event_type,
            payload,
        });
    }
    frames
}

/// Subscribe with `body`, assert the whole wire shape of a SERVED subscription, and answer the
/// one `SubscribeToShardEvent` it pushed.
async fn subscribed_event(emulator: &Emulator, case: &str, body: &Value) -> Value {
    let (status, headers, payload) = emulator.call_raw("SubscribeToShard", body).await;
    assert_eq!(
        status,
        200,
        "case {case}: expected SubscribeToShard to answer 200, got {status} with {:?}",
        String::from_utf8_lossy(&payload)
    );
    let media_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    assert_eq!(
        media_type, EVENT_STREAM,
        "case {case}: expected an event-stream body, got {media_type:?}"
    );
    let frames = decode_frames(case, &payload);
    let framed: Vec<(&str, &str)> = frames
        .iter()
        .map(|frame| (frame.message_type.as_str(), frame.event_type.as_str()))
        .collect();
    assert_eq!(
        framed,
        vec![("event", INITIAL_RESPONSE), ("event", DATA_EVENT)],
        "case {case}: a subscription opens with the frame the output shape arrives in and \
         then pushes one data event, got {framed:?}"
    );
    assert_eq!(
        frames[0].payload,
        json!({}),
        "case {case}: SubscribeToShardOutput's only member is the stream, so the initial \
         response carries none, got {}",
        frames[0].payload
    );
    frames[1].payload.clone()
}

/// The `Data` of every record one event delivered, decoded from the base64 the wire carries a
/// blob as, in the order the event delivered them.
fn delivered(case: &str, event: &Value) -> Vec<String> {
    event["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: an event carries Records, got {event}"))
        .iter()
        .map(|record| {
            let encoded = record["Data"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a record carries Data, got {record}"));
            let bytes = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case}: a record's Data is base64, got {error} for {encoded:?}")
            });
            String::from_utf8(bytes)
                .unwrap_or_else(|error| panic!("case {case}: a record's Data is text: {error}"))
        })
        .collect()
}

/// The refusal a request to `operation` was answered with: its code and its message, asserted
/// to be a client error at HTTP 400 — the status every modelled error of this operation
/// carries.
async fn refusal(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> (String, String) {
    let (status, response) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected {operation} to refuse with a modelled client error at 400, got \
         {status} with body {response}"
    );
    (error_code(&response), error_message(&response))
}

/// The `GetShardIterator` members that name the same position as the `StartingPosition`
/// `position` does: the two readers of a shard take the same five `ShardIteratorType` values,
/// each with the same value member under its own name.
fn iterator_members(position: &Value) -> Value {
    let mut members = json!({ "ShardIteratorType": position["Type"].clone() });
    for (from, to) in [
        ("SequenceNumber", "StartingSequenceNumber"),
        ("Timestamp", "Timestamp"),
    ] {
        if let Some(value) = position.get(from) {
            members[to] = value.clone();
        }
    }
    members
}

/// The records the PULL reader serves for the same stream, shard and position, as the payloads
/// it delivered: what the subscription's own records are compared against.
async fn pulled(
    emulator: &Emulator,
    case: &str,
    stream: &str,
    shard: &str,
    position: &Value,
) -> Vec<String> {
    let mut body = json!({ "StreamName": stream, "ShardId": shard });
    for (member, value) in iterator_members(position)
        .as_object()
        .expect("the iterator members are an object")
    {
        body[member] = value.clone();
    }
    let iterator = emulator.ok(case, "GetShardIterator", &body).await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetShardIterator answers a ShardIterator"))
        .to_string();
    let page = emulator
        .ok(case, "GetRecords", &json!({ "ShardIterator": iterator }))
        .await;
    delivered(case, &page)
}

/// A subscription delivers the records the shard retains FROM the position its request names,
/// whichever of the five positions that is, and reports the sequence number a consumer reads
/// on from — the same records the pull reader serves for that same position.
#[tokio::test]
async fn a_subscription_delivers_the_records_the_position_it_names_admits() {
    let emulator = Emulator::new();
    let stream = "efo-positions";
    active_stream(&emulator, "setup", stream, 1).await;
    let written = ["first", "second", "third"];
    let mut sequence_numbers = Vec::new();
    for payload in written {
        sequence_numbers.push(put(&emulator, "setup", stream, payload).await);
    }

    // (case, the StartingPosition the request names, the payloads it must deliver, the
    // ContinuationSequenceNumber the event must report). The positions and the member each of
    // them reads are the model's: `smithy_query(query="shape",shape="StartingPosition")`.
    let rows: Vec<(&str, Value, Vec<&str>, &str)> = vec![
        (
            "TRIM_HORIZON, the oldest record the shard retains",
            json!({ "Type": "TRIM_HORIZON" }),
            written.to_vec(),
            &sequence_numbers[2],
        ),
        (
            "AT_TIMESTAMP before the trim horizon, which reads from the oldest record",
            json!({ "Type": "AT_TIMESTAMP", "Timestamp": 0 }),
            written.to_vec(),
            &sequence_numbers[2],
        ),
        (
            "AT_SEQUENCE_NUMBER at the second record",
            json!({ "Type": "AT_SEQUENCE_NUMBER", "SequenceNumber": sequence_numbers[1] }),
            vec!["second", "third"],
            &sequence_numbers[2],
        ),
        (
            "AFTER_SEQUENCE_NUMBER after the first record",
            json!({ "Type": "AFTER_SEQUENCE_NUMBER", "SequenceNumber": sequence_numbers[0] }),
            vec!["second", "third"],
            &sequence_numbers[2],
        ),
        (
            // "LATEST: Start streaming just after the most recent record in the shard", so a
            // subscription started after every write delivers none of them — and still
            // reports the progress the consumer has made, which is what
            // `ContinuationSequenceNumber` is for ("it captures your shard progress even when
            // no data is written to the shard").
            "LATEST, just after the most recent record",
            json!({ "Type": "LATEST" }),
            Vec::new(),
            &sequence_numbers[2],
        ),
    ];

    for (index, (case, position, expected, continuation)) in rows.into_iter().enumerate() {
        // One consumer per row: a consumer that already holds a subscription to the shard is
        // refused a second one, which the table below is about.
        let consumer_arn =
            active_consumer(&emulator, case, stream, &format!("positions-{index}")).await;
        let event = subscribed_event(
            &emulator,
            case,
            &json!({
                "ConsumerARN": consumer_arn,
                "ShardId": FIRST_SHARD,
                "StartingPosition": position,
            }),
        )
        .await;

        let served = delivered(case, &event);
        assert_eq!(
            served, expected,
            "case {case:?}: expected the subscription to deliver {expected:?}, got {served:?}"
        );
        assert_eq!(
            event["ContinuationSequenceNumber"].as_str(),
            Some(continuation),
            "case {case:?}: expected the consumer to read on from {continuation}, got {event}"
        );
        let behind = event["MillisBehindLatest"].as_i64();
        assert!(
            behind.is_some_and(|millis| millis >= 0),
            "case {case:?}: expected a MillisBehindLatest of zero or more, got {event}"
        );
        assert!(
            event.get("ChildShards").is_none(),
            "case {case:?}: an open shard has no child shards to report, got {event}"
        );
        // The push reader and the pull reader are one rule about where a reader starts and
        // what it is served, so they deliver the same records for the same position.
        let pulled = pulled(&emulator, case, stream, FIRST_SHARD, &position).await;
        assert_eq!(
            served, pulled,
            "case {case:?}: expected the subscription to deliver what the pull reader serves \
             ({pulled:?}), got {served:?}"
        );
    }
}

/// A subscription naming a stream, a consumer or a shard this account and region do not hold
/// is refused with the modelled `ResourceNotFoundException`, stating the resource at fault.
#[tokio::test]
async fn a_subscription_naming_a_resource_that_is_not_there_is_refused() {
    let emulator = Emulator::new();
    let stream = "efo-missing";
    active_stream(&emulator, "setup", stream, 1).await;
    let consumer_arn = active_consumer(&emulator, "setup", stream, "missing-probe").await;

    // Another stream, so a case can name a shard that exists but belongs elsewhere.
    let other = "efo-missing-other";
    active_stream(&emulator, "setup", other, 2).await;
    let other_shard = "shardId-000000000001";

    // A `ConsumerARN` for a stream nothing created, and one for a consumer that was never
    // registered: both well inside the modelled pattern, so what refuses them is the state.
    let absent_stream = "efo-never-created";
    let absent_stream_consumer = format!(
        "{}/consumer/never-registered:1700000000",
        stream_arn(REGION, absent_stream)
    );
    let unregistered_consumer = format!(
        "{}/consumer/never-registered:1700000000",
        stream_arn(REGION, stream)
    );

    // (case, request body, the sentence the refusal must carry). The sentences are written
    // out here rather than borrowed from the crate, so the wording itself is what these cases
    // pin.
    let account = engine::EMULATOR_ACCOUNT_ID;
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a consumer ARN naming a stream this account does not hold",
            json!({
                "ConsumerARN": absent_stream_consumer,
                "ShardId": FIRST_SHARD,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            format!("Stream {absent_stream} under account {account} not found."),
        ),
        (
            "a consumer that was never registered with the stream it names",
            json!({
                "ConsumerARN": unregistered_consumer,
                "ShardId": FIRST_SHARD,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            format!(
                "Consumer never-registered with creation timestamp 1700000000 under stream: \
                 {stream}, account {account} not found."
            ),
        ),
        (
            "a shard id nothing laid down",
            json!({
                "ConsumerARN": consumer_arn,
                "ShardId": ABSENT_SHARD,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            format!(
                "Could not find shard {ABSENT_SHARD} in stream {stream} under account \
                 {account}."
            ),
        ),
        (
            "a shard of another stream",
            json!({
                "ConsumerARN": consumer_arn,
                "ShardId": other_shard,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            format!(
                "Could not find shard {other_shard} in stream {stream} under account \
                 {account}."
            ),
        ),
    ];

    for (case, body, sentence) in rows {
        let (code, message) = refusal(&emulator, case, "SubscribeToShard", &body).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code} with {message:?}"
        );
        assert_eq!(
            message, sentence,
            "case {case:?}: expected the refusal to state {sentence:?}, got {message:?}"
        );
    }
}

/// A consumer that already holds a subscription to a shard is refused a second one, while the
/// same consumer on another shard and another consumer on the same shard are both served.
#[tokio::test]
async fn a_second_subscription_of_one_consumer_to_one_shard_is_refused() {
    let emulator = Emulator::new();
    let stream = "efo-exclusive";
    active_stream(&emulator, "setup", stream, 2).await;
    let second_shard = "shardId-000000000001";
    let holder = active_consumer(&emulator, "setup", stream, "holder").await;
    let other = active_consumer(&emulator, "setup", stream, "other-holder").await;
    let position = json!({ "Type": "TRIM_HORIZON" });

    let subscribe = |consumer: String, shard: &'static str| {
        json!({
            "ConsumerARN": consumer,
            "ShardId": shard,
            "StartingPosition": position.clone(),
        })
    };

    subscribed_event(
        &emulator,
        "the first subscription",
        &subscribe(holder.clone(), FIRST_SHARD),
    )
    .await;

    let (code, message) = refusal(
        &emulator,
        "the same consumer and shard again",
        "SubscribeToShard",
        &subscribe(holder.clone(), FIRST_SHARD),
    )
    .await;
    assert_eq!(
        code, "ResourceInUseException",
        "case the same consumer and shard again: expected ResourceInUseException, got {code} \
         with {message:?}"
    );
    assert!(
        message.contains(&holder) && message.contains(FIRST_SHARD),
        "case the same consumer and shard again: expected the refusal to name the consumer \
         and the shard it already holds, got {message:?}"
    );

    // The subscription is held per consumer AND per shard, so neither of these is the one
    // that is already held.
    for (case, body) in [
        (
            "the same consumer on another shard",
            subscribe(holder, second_shard),
        ),
        (
            "another consumer on the same shard",
            subscribe(other, FIRST_SHARD),
        ),
    ] {
        subscribed_event(&emulator, case, &body).await;
    }
}

/// Once the window a subscription holds its shard for has passed, "the second call takes over
/// the subscription": the same consumer and shard are served again.
#[tokio::test]
async fn a_subscription_is_taken_over_once_the_window_has_passed() {
    let emulator = Emulator::new();
    let stream = "efo-takeover";
    active_stream(&emulator, "setup", stream, 1).await;
    let consumer_arn = active_consumer(&emulator, "setup", stream, "taking-over").await;
    let body = json!({
        "ConsumerARN": consumer_arn,
        "ShardId": FIRST_SHARD,
        "StartingPosition": { "Type": "TRIM_HORIZON" },
    });

    subscribed_event(&emulator, "the first subscription", &body).await;
    // The CONDITION the service reaches on its own clock — never a duration this case
    // measures: it looks again until the shard is served to this consumer once more.
    poll_until(
        "a taken-over subscription",
        "the same consumer and shard are served again",
        || async {
            let (status, _, _) = emulator.call_raw("SubscribeToShard", &body).await;
            status == 200
        },
    )
    .await;
}

/// A dry run is refused with the error the model declares for it and establishes nothing: the
/// real subscription that follows it is served.
#[tokio::test]
async fn a_dry_run_subscription_is_refused_and_establishes_nothing() {
    let emulator = Emulator::new();
    let stream = "efo-dry-run";
    active_stream(&emulator, "setup", stream, 1).await;
    let consumer_arn = active_consumer(&emulator, "setup", stream, "dry-runner").await;
    put(&emulator, "setup", stream, "payload").await;
    let body = json!({
        "ConsumerARN": consumer_arn,
        "ShardId": FIRST_SHARD,
        "StartingPosition": { "Type": "TRIM_HORIZON" },
        "DryRun": true,
    });

    let (code, message) = refusal(&emulator, "a dry run", "SubscribeToShard", &body).await;
    assert_eq!(
        code, "DryRunOperationException",
        "case a dry run: expected DryRunOperationException, got {code} with {message:?}"
    );
    assert_eq!(
        message,
        dry_run_succeeded_message("SubscribeToShard"),
        "case a dry run: expected the sentence this service states for a dry run, got \
         {message:?}"
    );

    let mut served = body.clone();
    served["DryRun"] = json!(false);
    let event = subscribed_event(&emulator, "after the dry run", &served).await;
    assert_eq!(
        delivered("after the dry run", &event),
        vec!["payload".to_string()],
        "case after the dry run: a dry run establishes no subscription, so the real one is \
         served, got {event}"
    );
}

/// The sentence a reader refuses a `Type` and a `SequenceNumber` that do not go together
/// with: the two legal combinations enumerated, then what the request named — written out
/// here rather than borrowed from the crate, so the wording itself is what these cases pin,
/// over the member THIS operation spells a sequence number with.
fn combination_refusal(position_type: &str, carried_sequence_number: bool) -> String {
    format!(
        "Must either specify (1) AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER and \
         StartingPosition.SequenceNumber or (2) TRIM_HORIZON or LATEST and no \
         StartingPosition.SequenceNumber. Request specified {position_type} and \
         {}StartingPosition.SequenceNumber.",
        if carried_sequence_number { "" } else { "no " }
    )
}

/// A starting position whose members its own `ShardIteratorType` does not admit is refused
/// with the modelled `InvalidArgumentException`, naming the member the request spelled.
#[tokio::test]
async fn a_starting_position_its_own_type_does_not_admit_is_refused() {
    let emulator = Emulator::new();
    let stream = "efo-positions-refused";
    active_stream(&emulator, "setup", stream, 1).await;
    let consumer_arn = active_consumer(&emulator, "setup", stream, "bad-positions").await;
    let sequence_number = put(&emulator, "setup", stream, "payload").await;

    // (case, the StartingPosition the request names, the sentence the refusal must carry).
    // The sentences are written out here rather than borrowed from the crate, so the wording
    // itself is what these cases pin.
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "AT_SEQUENCE_NUMBER without the sequence number it reads",
            json!({ "Type": "AT_SEQUENCE_NUMBER" }),
            combination_refusal("AT_SEQUENCE_NUMBER", false),
        ),
        (
            "AFTER_SEQUENCE_NUMBER without the sequence number it reads",
            json!({ "Type": "AFTER_SEQUENCE_NUMBER" }),
            combination_refusal("AFTER_SEQUENCE_NUMBER", false),
        ),
        (
            "AT_TIMESTAMP without the instant it reads",
            json!({ "Type": "AT_TIMESTAMP" }),
            "StartingPosition.Timestamp is required with the AT_TIMESTAMP shard iterator type"
                .to_string(),
        ),
        (
            "TRIM_HORIZON with a sequence number it does not read",
            json!({ "Type": "TRIM_HORIZON", "SequenceNumber": sequence_number }),
            combination_refusal("TRIM_HORIZON", true),
        ),
        (
            "LATEST with an instant it does not read",
            json!({ "Type": "LATEST", "Timestamp": 1_700_000_000 }),
            "StartingPosition.Timestamp can only be used with the AT_TIMESTAMP shard iterator \
             type, not with LATEST"
                .to_string(),
        ),
        (
            // Every sequence number a shard can hold lies inside the range it publishes
            // (`Shard$SequenceNumberRange`), so a value below it names a position in no shard.
            "a sequence number outside the shard's range",
            json!({ "Type": "AT_SEQUENCE_NUMBER", "SequenceNumber": "0" }),
            format!("StartingPosition.SequenceNumber 0 is not a sequence number of shard {FIRST_SHARD}"),
        ),
    ];

    for (case, position, sentence) in rows {
        let (code, message) = refusal(
            &emulator,
            case,
            "SubscribeToShard",
            &json!({
                "ConsumerARN": consumer_arn,
                "ShardId": FIRST_SHARD,
                "StartingPosition": position,
            }),
        )
        .await;
        assert_eq!(
            code, "InvalidArgumentException",
            "case {case:?}: expected InvalidArgumentException, got {code} with {message:?}"
        );
        assert_eq!(
            message, sentence,
            "case {case:?}: expected the refusal to state {sentence:?}, got {message:?}"
        );
    }
}

/// A request the MODEL's own constraints refuse is stated the way this service states one for
/// every other operation: HTTP 400 `ValidationException` with AWS's envelope over one clause
/// per broken constraint.
#[tokio::test]
async fn a_request_the_model_refuses_is_stated_in_this_services_words() {
    let emulator = Emulator::new();
    let stream = "efo-constraints";
    active_stream(&emulator, "setup", stream, 1).await;
    let consumer_arn = active_consumer(&emulator, "setup", stream, "constrained").await;

    // (case, request body, the member path the clause must name)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a consumer ARN outside its modelled pattern",
            json!({
                "ConsumerARN": "not-an-arn",
                "ShardId": FIRST_SHARD,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            "consumerARN",
        ),
        (
            "a shard id outside its modelled pattern",
            json!({
                "ConsumerARN": consumer_arn.clone(),
                "ShardId": "shards/one",
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            "shardId",
        ),
        (
            "a shard id past its modelled length",
            json!({
                "ConsumerARN": consumer_arn,
                "ShardId": "s".repeat(129),
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
            "shardId",
        ),
    ];

    for (case, body, member) in rows {
        let (code, message) = refusal(&emulator, case, "SubscribeToShard", &body).await;
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code} with {message:?}"
        );
        let clause = format!("at '{member}' failed to satisfy constraint:");
        assert!(
            message.starts_with("1 validation error detected: ") && message.contains(&clause),
            "case {case:?}: expected AWS's envelope over a clause naming {member}, got \
             {message:?}"
        );
    }
}

/// A subscription that has been delivered everything a CLOSED shard retains reports that
/// shard's children, which is where the consumer reads on: "The list of the child shards of
/// the current shard, returned only at the end of the current shard."
#[tokio::test]
async fn a_subscription_that_drains_a_closed_shard_reports_its_child_shards() {
    let emulator = Emulator::new();
    let stream = "efo-reshard";
    active_stream(&emulator, "setup", stream, 1).await;
    put(&emulator, "setup", stream, "before the split").await;
    emulator
        .ok(
            "setup",
            "SplitShard",
            &json!({
                "StreamName": stream,
                "ShardToSplit": FIRST_SHARD,
                "NewStartingHashKey": MIDDLE_HASH_KEY,
            }),
        )
        .await;
    poll_until_active(&emulator, "setup", stream).await;
    let consumer_arn = active_consumer(&emulator, "setup", stream, "resharded").await;

    let case = "a closed shard the subscription has drained";
    let event = subscribed_event(
        &emulator,
        case,
        &json!({
            "ConsumerARN": consumer_arn,
            "ShardId": FIRST_SHARD,
            "StartingPosition": { "Type": "TRIM_HORIZON" },
        }),
    )
    .await;
    assert_eq!(
        delivered(case, &event),
        vec!["before the split".to_string()],
        "case {case:?}: a closed shard still serves the records it retains, got {event}"
    );
    let children: Vec<String> = event["ChildShards"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ChildShards, got {event}"))
        .iter()
        .map(|child| {
            let parents: Vec<&str> = child["ParentShards"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a child names its parents, got {child}"))
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert_eq!(
                parents,
                vec![FIRST_SHARD],
                "case {case:?}: expected the split's child to name the shard it came from, got \
                 {child}"
            );
            assert!(
                child["HashKeyRange"]["StartingHashKey"].is_string()
                    && child["HashKeyRange"]["EndingHashKey"].is_string(),
                "case {case:?}: a child shard reports the hash key range it covers, got {child}"
            );
            child["ShardId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a child names its shard, got {child}"))
                .to_string()
        })
        .collect();
    assert_eq!(
        children.len(),
        2,
        "case {case:?}: a split produces two children for the shard it cut, got {children:?}"
    );
}
