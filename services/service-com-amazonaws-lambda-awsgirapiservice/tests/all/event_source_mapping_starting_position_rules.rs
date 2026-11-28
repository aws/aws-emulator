// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `CreateEventSourceMapping` answers: the lifecycle `State` the
//! accepted mapping carries, and the `StartingPosition` the event source it names can be
//! read from.
//!
//! Two rules under test:
//!
//! 1. The operation is ACCEPTED rather than applied — it answers 202 — so the mapping it
//!    answers is the one being stood up: `State` is `Creating` whatever polling the request
//!    asked for, and the move to the state that setting asks for ("When true, the event
//!    source mapping is active. When false, Lambda pauses polling and invocation. Default:
//!    True") is owed by the clock afterwards — `Enabled` for a mapping asked to be active,
//!    `Disabled` for one asked for with polling paused. The rows differ along how `Enabled`
//!    is stated (omitted, `true`, `false`) and which event source the mapping polls.
//! 2. A `StartingPosition` the request states has to be one the event source can be READ
//!    from — "the position in a stream from which to start reading", with `AT_TIMESTAMP`
//!    "supported only for Amazon Kinesis streams, Amazon DocumentDB, Amazon MSK, and
//!    self-managed Apache Kafka" (`smithy_query(query="doc",
//!    shape="CreateEventSourceMappingRequest", member="StartingPosition")`) — and a pairing
//!    of a source and a position outside that support is refused with the modelled
//!    `InvalidParameterValueException`, whose message is keyed on the ARN the request sent
//!    rather than on the position asked for. The rows pair each source the `EventSourceArn`
//!    member enumerates with positions it does and does not support, including sources the
//!    issue this suite was written for never cited.
//!
//! Every row drives real HTTP requests through the router — over the ONE mapping harness
//! this test binary has, [`crate::event_source_mappings`] — and reads each accepted mapping
//! back through `GetEventSourceMapping`, so a value that was answered but never stored
//! fails here.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::event_source_mappings::{
    await_mapping_state, create_function, create_mapping, create_request, created_state,
    get_mapping, router_on, Answer, NO_RECORDS_PROCESSED, STATE_DISABLED, STATE_ENABLED,
    USER_ACTION, USER_INITIATED,
};

/// The modelled refusal of a request this operation cannot accept
/// (`smithy_query(query="operation", operation="CreateEventSourceMapping")`:
/// `InvalidParameterValueException`, `@httpError(400)`).
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The values of `EventSourcePosition`, from `smithy_query(query="shape",
/// shape="EventSourcePosition")`: `TRIM_HORIZON | LATEST | AT_TIMESTAMP`.
const TRIM_HORIZON: &str = "TRIM_HORIZON";
/// See [`TRIM_HORIZON`].
const LATEST: &str = "LATEST";
/// See [`TRIM_HORIZON`].
const AT_TIMESTAMP: &str = "AT_TIMESTAMP";

/// An instant in the past, for the rows that read from one: "With StartingPosition set to
/// AT_TIMESTAMP, the time from which to start reading. StartingPositionTimestamp cannot be
/// in the future."
const PAST_INSTANT: i64 = 1_700_000_000;

/// The whole message a position the event source cannot be read from is refused with: the
/// fixed sentence, then the `EventSourceArn` the request sent, bare.
///
/// Stated here from the REQUEST, independently of the store's own rendering of the same
/// message, so that these assertions check the emulator rather than restate it.
fn unsupported_position_refusal(event_source: &str) -> String {
    format!("Unsupported starting position for arn type: {event_source}")
}

/// One event source of a row, as the `EventSourceArn` member enumerates them: each row gets
/// its OWN resource of that kind, because two mappings of the same function over the same
/// records conflict.
fn kinesis_stream(name: &str) -> String {
    format!("arn:aws:kinesis:us-east-1:000000000000:stream/{name}")
}

/// See [`kinesis_stream`]: "Amazon DynamoDB Streams – The ARN of the stream."
fn dynamodb_stream(name: &str) -> String {
    format!("arn:aws:dynamodb:us-east-1:000000000000:table/{name}/stream/2026-10-23T11:46:49.000")
}

/// See [`kinesis_stream`]: "Amazon Managed Streaming for Apache Kafka – The ARN of the
/// cluster."
fn msk_cluster(name: &str) -> String {
    format!(
        "arn:aws:kafka:us-east-1:000000000000:cluster/{name}/11111111-2222-3333-4444-555555555555-1"
    )
}

/// See [`kinesis_stream`]: "Amazon DocumentDB – The ARN of the DocumentDB change stream",
/// whose ARNs are in the `rds` namespace.
fn docdb_cluster(name: &str) -> String {
    format!("arn:aws:rds:us-east-1:000000000000:cluster:{name}")
}

/// See [`kinesis_stream`]: "Amazon Simple Queue Service – The ARN of the queue."
fn queue(name: &str) -> String {
    format!("arn:aws:sqs:us-east-1:000000000000:{name}")
}

/// See [`kinesis_stream`]: "Amazon MQ – The ARN of the broker."
fn mq_broker(name: &str) -> String {
    format!("arn:aws:mq:us-east-1:000000000000:broker:{name}:b-1111")
}

/// What a position row expects of the create.
enum Outcome {
    /// The position is one the source is read from: the mapping is created and reports it.
    Accepted,
    /// The source cannot be read from that position: the modelled 400, naming the ARN.
    Refused,
}

/// One position row: the source, the position stated beside it, and what must happen.
struct PositionRow {
    case: &'static str,
    /// `EventSourceArn`, or `None` for the self-managed Apache Kafka cluster, which carries
    /// no ARN at all.
    event_source: Option<String>,
    /// The `StartingPosition` the request states.
    position: &'static str,
    /// What the request states beside the function, the source and the position.
    extras: Value,
    outcome: Outcome,
}

/// The rows: every source the `EventSourceArn` member enumerates, paired with positions it
/// is and is not read from — each of the three enum values on the sources that support it,
/// and reading from a stated instant on three ARN types that do not.
fn position_rows() -> Vec<PositionRow> {
    vec![
        PositionRow {
            case: "a kinesis data stream read from the latest records",
            event_source: Some(kinesis_stream("pos-kinesis-latest")),
            position: LATEST,
            extras: json!({}),
            outcome: Outcome::Accepted,
        },
        PositionRow {
            case: "a kinesis data stream read from the oldest retained records",
            event_source: Some(kinesis_stream("pos-kinesis-horizon")),
            position: TRIM_HORIZON,
            extras: json!({}),
            outcome: Outcome::Accepted,
        },
        PositionRow {
            case: "a kinesis data stream read from a stated instant",
            event_source: Some(kinesis_stream("pos-kinesis-instant")),
            position: AT_TIMESTAMP,
            extras: json!({ "StartingPositionTimestamp": PAST_INSTANT }),
            outcome: Outcome::Accepted,
        },
        PositionRow {
            case: "a dynamodb stream read from the oldest retained records",
            event_source: Some(dynamodb_stream("pos-ddb-horizon")),
            position: TRIM_HORIZON,
            extras: json!({}),
            outcome: Outcome::Accepted,
        },
        PositionRow {
            case: "a dynamodb stream read from the latest records",
            event_source: Some(dynamodb_stream("pos-ddb-latest")),
            position: LATEST,
            extras: json!({}),
            outcome: Outcome::Accepted,
        },
        // The one pairing of the two the position's own documentation leaves out: a
        // DynamoDB stream is not among the sources `AT_TIMESTAMP` is supported for.
        PositionRow {
            case: "a dynamodb stream read from a stated instant",
            event_source: Some(dynamodb_stream("pos-ddb-instant")),
            position: AT_TIMESTAMP,
            extras: json!({ "StartingPositionTimestamp": PAST_INSTANT }),
            outcome: Outcome::Refused,
        },
        PositionRow {
            case: "an msk cluster read from a stated instant",
            event_source: Some(msk_cluster("pos-msk-instant")),
            position: AT_TIMESTAMP,
            extras: json!({
                "StartingPositionTimestamp": PAST_INSTANT,
                "Topics": ["order-events"],
            }),
            outcome: Outcome::Accepted,
        },
        PositionRow {
            case: "a documentdb change stream read from a stated instant",
            event_source: Some(docdb_cluster("pos-docdb-instant")),
            position: AT_TIMESTAMP,
            extras: json!({
                "StartingPositionTimestamp": PAST_INSTANT,
                "DocumentDBEventSourceConfig": { "DatabaseName": "orders" },
            }),
            outcome: Outcome::Accepted,
        },
        PositionRow {
            case: "a self-managed kafka cluster read from a stated instant",
            event_source: None,
            position: AT_TIMESTAMP,
            extras: json!({
                "StartingPositionTimestamp": PAST_INSTANT,
                "SelfManagedEventSource": {
                    "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-9.example.com:9092"] },
                },
                "Topics": ["order-events"],
            }),
            outcome: Outcome::Accepted,
        },
        // A queue and a broker queue hand their consumer the messages they hold: neither is
        // among the sources reading from a stated instant is supported for either.
        PositionRow {
            case: "a queue read from a stated instant",
            event_source: Some(queue("pos-queue-instant")),
            position: AT_TIMESTAMP,
            extras: json!({ "StartingPositionTimestamp": PAST_INSTANT }),
            outcome: Outcome::Refused,
        },
        PositionRow {
            case: "an mq broker read from a stated instant",
            event_source: Some(mq_broker("pos-broker-instant")),
            position: AT_TIMESTAMP,
            extras: json!({
                "StartingPositionTimestamp": PAST_INSTANT,
                "Queues": ["order-jobs"],
            }),
            outcome: Outcome::Refused,
        },
    ]
}

/// The create body a position row sends.
fn position_request(function: &str, row: &PositionRow) -> Value {
    let mut extras = row.extras.clone();
    if let Some(members) = extras.as_object_mut() {
        members.insert("StartingPosition".to_string(), json!(row.position));
    }
    create_request(function, row.event_source.as_deref(), &extras)
}

#[tokio::test]
async fn a_stated_starting_position_is_held_to_the_event_source_it_names() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-position-fn";
    create_function(&router, function).await;

    for row in position_rows() {
        let case = row.case;
        let answered = create_mapping(&router, &position_request(function, &row)).await;
        match row.outcome {
            Outcome::Accepted => {
                assert_eq!(
                    answered.status,
                    202,
                    "case {case:?}: the source is read from {}, so the create is accepted, \
                     got {} {}",
                    row.position,
                    answered.status,
                    answered.text()
                );
                assert_eq!(
                    answered.string("StartingPosition"),
                    Some(row.position.to_string()),
                    "case {case:?}: the create must report StartingPosition {}, got {}",
                    row.position,
                    answered.text()
                );
                // The round trip: the position comes back out of the store.
                let uuid = answered.string("UUID").unwrap_or_else(|| {
                    panic!("case {case:?}: the create answers a UUID, got {}", answered.text())
                });
                let fetched = get_mapping(&router, &uuid).await;
                assert_eq!(
                    (fetched.status, fetched.string("StartingPosition")),
                    (200, Some(row.position.to_string())),
                    "case {case:?}: the read-back must report StartingPosition {}, got {} {}",
                    row.position,
                    fetched.status,
                    fetched.text()
                );
            }
            Outcome::Refused => {
                let event_source = row.event_source.as_deref().unwrap_or_default();
                assert_eq!(
                    answered.status,
                    400,
                    "case {case:?}: the source cannot be read from {}, so the create is \
                     refused, got {} {}",
                    row.position,
                    answered.status,
                    answered.text()
                );
                assert!(
                    answered.error_code().contains(INVALID_PARAMETER),
                    "case {case:?}: expected {INVALID_PARAMETER}, got {}",
                    answered.error_code()
                );
                assert_eq!(
                    answered.string("message"),
                    Some(unsupported_position_refusal(event_source)),
                    "case {case:?}: expected the refusal to name the ARN the request sent, \
                     got {}",
                    answered.text()
                );
            }
        }
    }
}

/// One lifecycle row: how the request states its polling setting, and what the mapping must
/// settle into once the clock makes the move due.
struct LifecycleRow {
    case: &'static str,
    event_source: String,
    /// What the request states beside the function and the event source.
    extras: Value,
    /// The terminal `State` the clock settles the mapping into.
    settles_into: &'static str,
    /// `StateTransitionReason` for a change this mapping's caller asked for, and the
    /// `LastProcessingResult` it reports before its poller has attempted a batch — which is
    /// the source's own reading (`None` for a source that reports none at all).
    reason: &'static str,
    unattempted: Option<&'static str>,
    /// `TumblingWindowInSeconds`: "(Kinesis and DynamoDB Streams only) The duration in
    /// seconds of a processing window ... A value of 0 seconds indicates no tumbling
    /// window" — so a mapping on one of those two sources whose request stated none has no
    /// window, which is that duration, and no other source reports the member at all.
    tumbling_window: Option<i64>,
}

/// The rows: the three ways the polling setting is stated, each on a source whose readings
/// of the other lifecycle members differ.
fn lifecycle_rows() -> Vec<LifecycleRow> {
    vec![
        LifecycleRow {
            case: "a dynamodb stream mapping asked for with polling paused",
            event_source: dynamodb_stream("life-ddb-paused"),
            extras: json!({ "Enabled": false, "StartingPosition": TRIM_HORIZON }),
            settles_into: STATE_DISABLED,
            reason: USER_ACTION,
            unattempted: Some(NO_RECORDS_PROCESSED),
            tumbling_window: Some(0),
        },
        LifecycleRow {
            case: "a kinesis mapping asked to be active",
            event_source: kinesis_stream("life-kinesis-active"),
            extras: json!({ "Enabled": true, "StartingPosition": LATEST }),
            settles_into: STATE_ENABLED,
            reason: USER_ACTION,
            unattempted: Some(NO_RECORDS_PROCESSED),
            tumbling_window: Some(0),
        },
        LifecycleRow {
            case: "a queue mapping stating no polling setting at all",
            event_source: queue("life-queue-default"),
            extras: json!({}),
            settles_into: STATE_ENABLED,
            reason: USER_INITIATED,
            unattempted: None,
            tumbling_window: None,
        },
        LifecycleRow {
            case: "a queue mapping asked for with polling paused",
            event_source: queue("life-queue-paused"),
            extras: json!({ "Enabled": false }),
            settles_into: STATE_DISABLED,
            reason: USER_INITIATED,
            unattempted: None,
            tumbling_window: None,
        },
    ]
}

/// The `DestinationConfig` of a mapping on one of the stream sources carries the failure
/// slot with no destination in it until one is stated, and no other source has one at all:
/// "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) A configuration
/// object that specifies the destination of an event after Lambda processes it."
fn failure_slot(answered: &Answer) -> Option<Value> {
    answered
        .json()
        .get("DestinationConfig")
        .and_then(|config| config.get("OnFailure"))
        .cloned()
}

#[tokio::test]
async fn every_accepted_create_is_transitional_and_settles_into_its_polling_setting() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "esm-lifecycle-fn";
    create_function(&router, function).await;

    // Every row is created first, over the one store, so the clock-owed transitions of all
    // of them run down together.
    let mut created = Vec::new();
    for row in lifecycle_rows() {
        let case = row.case;
        let answered = create_mapping(
            &router,
            &create_request(function, Some(&row.event_source), &row.extras),
        )
        .await;
        assert_eq!(
            answered.status,
            202,
            "case {case:?}: the create must answer 202 Accepted, got {} {}",
            answered.status,
            answered.text()
        );
        // A create is accepted rather than applied, so it answers the transitional state
        // whatever polling the row asked for; the state that setting asks for is the
        // clock's to bring.
        let created_state = created_state(&row.extras);
        assert_eq!(
            (
                answered.string("State"),
                answered.string("StateTransitionReason")
            ),
            (
                Some(created_state.to_string()),
                Some(row.reason.to_string())
            ),
            "case {case:?}: expected State {created_state} and StateTransitionReason {}, \
             got {}",
            row.reason,
            answered.text()
        );
        assert_eq!(
            answered.string("LastProcessingResult").as_deref(),
            row.unattempted,
            "case {case:?}: no poller has attempted a batch, so the mapping reports \
             {:?}, got {}",
            row.unattempted,
            answered.text()
        );
        assert_eq!(
            answered
                .json()
                .get("TumblingWindowInSeconds")
                .and_then(Value::as_i64),
            row.tumbling_window,
            "case {case:?}: expected TumblingWindowInSeconds {:?}, got {}",
            row.tumbling_window,
            answered.text()
        );
        let uuid = answered.string("UUID").unwrap_or_else(|| {
            panic!(
                "case {case:?}: the create answers a UUID, got {}",
                answered.text()
            )
        });
        created.push((row, uuid, failure_slot(&answered)));
    }

    for (row, uuid, created_slot) in created {
        let case = row.case;
        // The move out of the create's window is owed by the CLOCK, so this waits for the
        // state it brings — looking again until it holds or the crate's own budget for
        // clock-owed work passes ([`await_mapping_state`]) — rather than for a span of wall
        // clock.
        await_mapping_state(&router, case, &uuid, row.settles_into).await;
        let settled = get_mapping(&router, &uuid).await;
        assert_eq!(
            (settled.status, settled.string("State")),
            (200, Some(row.settles_into.to_string())),
            "case {case:?}: once the settle was due the mapping must report {}, got {} {}",
            row.settles_into,
            settled.status,
            settled.text()
        );
        assert_eq!(
            (
                settled.string("StateTransitionReason"),
                failure_slot(&settled)
            ),
            (Some(row.reason.to_string()), created_slot.clone()),
            "case {case:?}: the settled mapping reports the same reason and destination \
             configuration the create did ({:?}), got {}",
            created_slot,
            settled.text()
        );
    }
}
