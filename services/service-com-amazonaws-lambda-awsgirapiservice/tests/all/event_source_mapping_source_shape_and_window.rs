// SPDX-License-Identifier: CC0-1.0

//! What `CreateEventSourceMapping` tells a caller whose request names the wrong SHAPE of
//! event source ARN or leaves out a member the source it named needs, and the processing
//! window a mapping on a shard source reports when its request stated none — over the wire,
//! read back through `GetEventSourceMapping` and `ListEventSourceMappings`.
//!
//! Three rules are asserted here, each over a table whose rows differ along what the rule
//! generalises over:
//!
//! 1. An `EventSourceArn` whose vendor namespace IS one of the source services that member
//!    enumerates ("Amazon Kinesis – The ARN of the data stream or a stream consumer", "Amazon
//!    DynamoDB Streams – The ARN of the stream", "Amazon Simple Queue Service – The ARN of
//!    the queue", "Amazon Managed Streaming for Apache Kafka – The ARN of the cluster …",
//!    "Amazon MQ – The ARN of the broker", "Amazon DocumentDB – The ARN of the DocumentDB
//!    change stream") but whose resource is not the shape that service's pollable resource is
//!    addressed by is refused with the modelled `InvalidParameterValueException` stating the
//!    ARN FORMAT expected for that source service, written for the Region the request was
//!    made in; an ARN of any other vendor names no source service at all, so the refusal
//!    states that the source ARN sent is unsupported instead. The rows differ along the
//!    vendor, the resource type and the separator the resource field uses, the Region and
//!    account the value names, and include vendors outside that enumeration.
//!
//! 2. A member the model leaves OPTIONAL but this operation requires for the event source or
//!    the starting position the request names — `StartingPosition`, "Required for Amazon
//!    Kinesis and Amazon DynamoDB Stream event sources", and `StartingPositionTimestamp`,
//!    "With StartingPosition set to AT_TIMESTAMP, the time from which to start reading" — is
//!    refused, when absent, as the request validation error it is: AWS's envelope over that
//!    member's lowerCamel path. The rows differ along which member is absent, which source
//!    the mapping names (each source that needs it and sources that do not) and which
//!    starting position it asks for.
//!
//! 3. `TumblingWindowInSeconds` is "(Kinesis and DynamoDB Streams only) The duration in
//!    seconds of a processing window for DynamoDB and Kinesis Streams event sources. A value
//!    of 0 seconds indicates no tumbling window", so a mapping on one of those two sources
//!    whose request stated no window reports that duration — 0 — rather than no value, and a
//!    mapping on any other source reports the member not at all. The rows differ along the
//!    source, whether a window is stated, and the window's value: both published bounds of
//!    `@range(min: 0, max: 900)`, a value between them, and one past each bound.
//!
//! The member names, the per-source scopes and the range above are the model's own
//! (`smithy_query(query="shape", shape="com.amazonaws.lambda#EventSourceMappingConfiguration")`
//! and `smithy_query(query="doc", shape="CreateEventSourceMappingRequest",
//! member="EventSourceArn")`).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    create_function, create_mapping, create_request, get_mapping, given_broker, list_mappings,
    router_on, unsupported_source_refusal, DYNAMODB_SOURCE, KINESIS_SOURCE, MQ_BROKER_NAME,
    MSK_SOURCE, QUEUE_SOURCE,
};

/// The modelled error a request no mapping can satisfy is refused with
/// (`InvalidParameterValueException`, HTTP 400).
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// An Amazon MQ broker, "Amazon MQ – The ARN of the broker" — whose resource field uses
/// colons rather than slashes.
const MQ_SOURCE: &str = "arn:aws:mq:us-east-1:000000000000:broker:order-broker:b-1111";

/// A DocumentDB change stream, "Amazon DocumentDB – The ARN of the DocumentDB change
/// stream", which is an Amazon RDS cluster.
const DOCDB_SOURCE: &str = "arn:aws:rds:us-east-1:000000000000:cluster:order-docdb";

/// The published bounds of `TumblingWindowInSeconds` (`@range(min: 0, max: 900)`).
const WINDOW_MIN: i64 = 0;
/// See [`WINDOW_MIN`].
const WINDOW_MAX: i64 = 900;

/// Lambda alone: every rule here is about what the control plane ACCEPTS and REPORTS, so no
/// stream, queue or cluster has to exist for it.
fn emulator() -> MultiServiceRouter {
    router_on(&Arc::new(engine::InMemoryEngine::new()))
}

/// The `SelfManagedEventSource` of a self-managed Apache Kafka cluster, the one event source
/// named instead of an `EventSourceArn`.
fn self_managed_cluster() -> Value {
    json!({
        "SelfManagedEventSource": {
            "Endpoints": { "KAFKA_BOOTSTRAP_SERVERS": ["broker-1.example.com:9092"] },
        },
        "Topics": ["order-events"],
    })
}

/// The Region the harness signs for, which the ARN format a refusal states is written for.
const REGION: &str = "us-east-1";

/// The account field of an ARN written as a FORMAT: the example id every such format carries
/// beside the placeholder names of its resource half, rather than an account anything lives
/// in.
const EXAMPLE_ACCOUNT: &str = "123456789012";

/// The whole refusal an ARN of the wrong shape for the source service it names answers:
/// the ARN format that service's pollable resource is addressed by, written for the Region
/// the request was made in.
///
/// `pub(crate)` because this sentence has ONE owner here: the suite over which values name one
/// stream of one table reads it from this function too
/// (`tests/all/event_source_mapping_stream_arn_resolution.rs`), so the two tables cannot drift
/// apart.
pub(crate) fn expected_format_refusal(vendor: &str, resource: &str) -> String {
    format!(
        "Unrecognized event source. Expected arn of format \
         arn:aws:{vendor}:{REGION}:{EXAMPLE_ACCOUNT}:{resource}"
    )
}

/// What the refusal of one wrong-shaped ARN must state.
enum Stated {
    /// The ARN names one of the source services, so the refusal states the format that
    /// service's pollable resource is addressed by: that service's vendor namespace and the
    /// resource half of the format.
    ExpectedFormat(&'static str, &'static str),
    /// The ARN names no source service, so the refusal states that the source ARN sent is
    /// unsupported.
    UnsupportedArn,
}

/// One row of the ARN-shape table: the value `EventSourceArn` carries and what the refusal
/// of it must state.
struct ShapeRow {
    case: &'static str,
    event_source: &'static str,
    /// The members a mapping on that vendor would need beside its source, so the row is
    /// refused on the ARN's shape rather than on anything else the request lacks.
    extras: Value,
    stated: Stated,
}

/// The rows: one wrong resource shape per vendor `EventSourceArn` enumerates — a DynamoDB
/// table and a table's backup rather than a stream, a Kinesis application rather than a data
/// stream, a queue path rather than a queue name, an Amazon MSK configuration rather than a
/// cluster, an Amazon MQ configuration rather than a broker and an RDS database instance
/// rather than a cluster — plus the three ways a DynamoDB `table/…/stream/…` value still names
/// no one stream of one table (a label carrying a further segment, no label, no table), a
/// table of another Region and account and one naming no account at all, and three vendors
/// that enumeration does not name.
fn shape_rows() -> Vec<ShapeRow> {
    vec![
        ShapeRow {
            case: "a dynamodb table rather than its stream",
            event_source: "arn:aws:dynamodb:us-east-1:000000000000:table/order-intake",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            case: "a dynamodb table of another Region and account rather than its stream",
            event_source: "arn:aws:dynamodb:eu-west-1:123456789012:table/order-intake",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            case: "a dynamodb table naming no account rather than its stream",
            event_source: "arn:aws:dynamodb:us-east-1::table/order-intake",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            case: "a dynamodb table's backup rather than its stream",
            event_source:
                "arn:aws:dynamodb:us-east-1:000000000000:table/order-intake/backup/01700000000000-a1b2c3d4",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            // The label identifies ONE stream of the table, so a label carrying a further
            // segment addresses something other than one stream of one table — the shape the
            // service that owns those streams judges its own identifiers by too.
            case: "a dynamodb stream label carrying a further segment",
            event_source:
                "arn:aws:dynamodb:us-east-1:000000000000:table/order-intake/stream/label/nonexistent-stream-label",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            case: "a dynamodb stream naming no label at all",
            event_source: "arn:aws:dynamodb:us-east-1:000000000000:table/order-intake/stream/",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            case: "a dynamodb stream naming no table at all",
            event_source:
                "arn:aws:dynamodb:us-east-1:000000000000:table//stream/2026-10-23T11:46:49.000",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("dynamodb", "table/TableName/stream/StreamId"),
        },
        ShapeRow {
            case: "a kinesis analytics application rather than a data stream",
            event_source: "arn:aws:kinesis:us-east-1:000000000000:application/order-events",
            extras: json!({ "StartingPosition": "LATEST" }),
            stated: Stated::ExpectedFormat("kinesis", "stream/StreamName"),
        },
        ShapeRow {
            case: "a queue addressed by a path rather than by its name",
            event_source: "arn:aws:sqs:us-east-1:000000000000:queues/order-intake",
            extras: json!({}),
            stated: Stated::ExpectedFormat("sqs", "QueueName"),
        },
        ShapeRow {
            case: "an msk configuration rather than a cluster",
            event_source:
                "arn:aws:kafka:us-east-1:000000000000:configuration/order-config/11111111-2222-3333-4444-555555555555-1",
            extras: json!({ "Topics": ["order-events"] }),
            stated: Stated::ExpectedFormat("kafka", "cluster/ClusterName/ClusterId"),
        },
        ShapeRow {
            case: "an mq configuration rather than a broker",
            event_source: "arn:aws:mq:us-east-1:000000000000:configuration:order-config:c-1111",
            extras: json!({ "Queues": ["order-jobs"] }),
            // The resource half is written with the placeholder names the recorded format
            // uses, which for a broker are lowerCamel.
            stated: Stated::ExpectedFormat("mq", "broker:brokerName:brokerId"),
        },
        ShapeRow {
            case: "a database instance rather than a documentdb cluster",
            event_source: "arn:aws:rds:us-east-1:000000000000:db:order-db",
            extras: json!({}),
            stated: Stated::ExpectedFormat("rds", "cluster:ClusterName"),
        },
        ShapeRow {
            case: "a bucket, whose vendor is no event source service",
            event_source: "arn:aws:s3:::order-dropbox",
            extras: json!({}),
            stated: Stated::UnsupportedArn,
        },
        ShapeRow {
            case: "a topic, whose vendor is no event source service",
            event_source: "arn:aws:sns:us-east-1:000000000000:order-events",
            extras: json!({}),
            stated: Stated::UnsupportedArn,
        },
        ShapeRow {
            case: "a function, whose vendor is no event source service",
            event_source: "arn:aws:lambda:us-east-1:000000000000:function:order-fn",
            extras: json!({}),
            stated: Stated::UnsupportedArn,
        },
    ]
}

/// RULE: an `EventSourceArn` that names one of the source services `EventSourceArn`
/// enumerates, but not the shape of resource that service is polled through, is refused with
/// the modelled 400 stating the ARN format expected for THAT service; an ARN of any other
/// vendor is refused as an unsupported source ARN. Neither stores a mapping.
#[tokio::test]
async fn a_wrong_shaped_event_source_arn_is_refused_with_the_format_its_service_expects() {
    let router = emulator();
    for (index, row) in shape_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("esm-shape-{index}");
        create_function(&router, &function).await;

        let answer = create_mapping(
            &router,
            &create_request(&function, Some(row.event_source), &row.extras),
        )
        .await;

        assert_eq!(
            answer.status,
            400,
            "case {case:?}: {} names no event source a mapping can poll, so the create must \
             answer 400, got {} {}",
            row.event_source,
            answer.status,
            answer.text()
        );
        let code = answer.error_code();
        assert!(
            code.contains(INVALID_PARAMETER),
            "case {case:?}: the refusal must be {INVALID_PARAMETER}, got {code:?}"
        );

        let message = answer.string("message").unwrap_or_default();
        match row.stated {
            Stated::ExpectedFormat(vendor, resource) => assert_eq!(
                message,
                expected_format_refusal(vendor, resource),
                "case {case:?}: the refusal must state the ARN format this source service is \
                 polled through, got {message:?}"
            ),
            Stated::UnsupportedArn => assert_eq!(
                message,
                unsupported_source_refusal(row.event_source),
                "case {case:?}: the refusal must state that the source ARN sent is \
                 unsupported, got {message:?}"
            ),
        }

        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();
        assert_eq!(
            held,
            0,
            "case {case:?}: a refused create must store no mapping, got {held} mapping(s) in {}",
            listed.text()
        );
    }
}

/// The lowerCamel member paths AWS's validation error names the two conditionally required
/// members by, from the model's own member names (`StartingPosition`,
/// `StartingPositionTimestamp`).
const STARTING_POSITION_PATH: &str = "startingPosition";
/// See [`STARTING_POSITION_PATH`].
const STARTING_POSITION_TIMESTAMP_PATH: &str = "startingPositionTimestamp";

/// See [`STARTING_POSITION_PATH`]: the member a mapping onto a DocumentDB cluster
/// requires, nested in the structure it arrives in (`DocumentDBEventSourceConfig`,
/// `DatabaseName`).
const DOCUMENT_DB_DATABASE_NAME_PATH: &str = "documentDBEventSourceConfig.databaseName";

/// The whole refusal a request that left out a required member answers: AWS's
/// validation-error envelope over that member's path.
fn absent_member_refusal(member_path: &str) -> String {
    format!(
        "1 validation error detected: Value null at '{member_path}' failed to satisfy \
         constraint: Member must not be null."
    )
}

/// One row of the required-member table: what the request states, and either the member path
/// its refusal must name or nothing at all, for a request the operation accepts.
struct MemberRow {
    case: &'static str,
    /// `EventSourceArn`, or `None` for the self-managed Apache Kafka cluster.
    event_source: Option<&'static str>,
    extras: Value,
    /// The member path the refusal must name, and `None` for a request that needs no member
    /// it left out.
    absent: Option<&'static str>,
}

/// The rows: each of the two sources `StartingPosition` is "Required for" with and without
/// one, each source that does NOT require it, and the position that requires a timestamp with
/// and without one.
fn member_rows() -> Vec<MemberRow> {
    vec![
        MemberRow {
            case: "a kinesis data stream with no starting position",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({}),
            absent: Some(STARTING_POSITION_PATH),
        },
        MemberRow {
            case: "a dynamodb stream with no starting position",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({ "BatchSize": 1 }),
            absent: Some(STARTING_POSITION_PATH),
        },
        MemberRow {
            case: "a kinesis data stream read from a timestamp that is not stated",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({ "StartingPosition": "AT_TIMESTAMP" }),
            absent: Some(STARTING_POSITION_TIMESTAMP_PATH),
        },
        MemberRow {
            case: "a documentdb change stream read from a timestamp that is not stated",
            event_source: Some(DOCDB_SOURCE),
            extras: json!({ "StartingPosition": "AT_TIMESTAMP" }),
            absent: Some(STARTING_POSITION_TIMESTAMP_PATH),
        },
        MemberRow {
            case: "a kinesis data stream read from the latest records",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({ "StartingPosition": "LATEST" }),
            absent: None,
        },
        MemberRow {
            case: "a dynamodb stream read from the oldest retained records",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({ "StartingPosition": "TRIM_HORIZON" }),
            absent: None,
        },
        MemberRow {
            case: "a kinesis data stream read from a stated timestamp",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({
                "StartingPosition": "AT_TIMESTAMP",
                "StartingPositionTimestamp": 1_700_000_000,
            }),
            absent: None,
        },
        MemberRow {
            case: "a queue, which needs no starting position",
            event_source: Some(QUEUE_SOURCE),
            extras: json!({}),
            absent: None,
        },
        MemberRow {
            case: "an msk cluster, which needs no starting position",
            event_source: Some(MSK_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            absent: None,
        },
        MemberRow {
            case: "an mq broker, which needs no starting position",
            event_source: Some(MQ_SOURCE),
            extras: json!({ "Queues": ["order-jobs"] }),
            absent: None,
        },
        MemberRow {
            case: "a self-managed kafka cluster, which needs no starting position",
            event_source: None,
            extras: self_managed_cluster(),
            absent: None,
        },
        // A DocumentDB change stream is consumed inside ONE database of the cluster, so a
        // mapping onto one requires the name of that database wherever the rest of the
        // request is acceptable.
        MemberRow {
            case: "a documentdb change stream naming no database",
            event_source: Some(DOCDB_SOURCE),
            extras: json!({}),
            absent: Some(DOCUMENT_DB_DATABASE_NAME_PATH),
        },
        MemberRow {
            case: "a documentdb change stream whose configuration names no database",
            event_source: Some(DOCDB_SOURCE),
            extras: json!({ "DocumentDBEventSourceConfig": { "CollectionName": "orders" } }),
            absent: Some(DOCUMENT_DB_DATABASE_NAME_PATH),
        },
        MemberRow {
            case: "a documentdb change stream naming the database it consumes",
            event_source: Some(DOCDB_SOURCE),
            extras: json!({ "DocumentDBEventSourceConfig": { "DatabaseName": "orders" } }),
            absent: None,
        },
        MemberRow {
            case: "a documentdb change stream read from a stated timestamp",
            event_source: Some(DOCDB_SOURCE),
            extras: json!({
                "StartingPosition": "AT_TIMESTAMP",
                "StartingPositionTimestamp": 1_700_000_000,
                "DocumentDBEventSourceConfig": { "DatabaseName": "orders" },
            }),
            absent: None,
        },
    ]
}

/// RULE: a member this operation requires for the source or the position a request names,
/// though the model leaves it optional, is refused when absent as a request validation error
/// — AWS's envelope over that member's own path — and nothing is stored; a request that needs
/// no member it left out is accepted.
#[tokio::test]
async fn a_member_required_for_the_named_source_is_refused_as_a_validation_error_when_absent() {
    let router = emulator();
    // The broker the Amazon MQ row names: a mapping onto a broker is accepted only against
    // one Amazon MQ itself holds.
    given_broker(&router, MQ_BROKER_NAME).await;
    for (index, row) in member_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("esm-member-{index}");
        create_function(&router, &function).await;

        let answer = create_mapping(
            &router,
            &create_request(&function, row.event_source, &row.extras),
        )
        .await;
        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let held = listed.mappings().len();

        match row.absent {
            Some(member_path) => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: this request leaves out {member_path}, so the create must \
                     answer 400, got {} {}",
                    answer.status,
                    answer.text()
                );
                let code = answer.error_code();
                assert!(
                    code.contains(INVALID_PARAMETER),
                    "case {case:?}: the refusal must be {INVALID_PARAMETER}, got {code:?}"
                );
                let message = answer.string("message").unwrap_or_default();
                assert_eq!(
                    message,
                    absent_member_refusal(member_path),
                    "case {case:?}: the refusal must name {member_path} as the member that \
                     must not be null, got {message:?}"
                );
                assert_eq!(
                    held, 0,
                    "case {case:?}: a refused create must store no mapping, got {held} \
                     mapping(s) in {}",
                    listed.text()
                );
            }
            None => {
                assert_eq!(
                    answer.status,
                    202,
                    "case {case:?}: this request states every member its source needs, so the \
                     create must answer 202, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    held, 1,
                    "case {case:?}: the accepted mapping must be held, got {held} mapping(s) \
                     in {}",
                    listed.text()
                );
            }
        }
    }
}

/// What a mapping must report for `TumblingWindowInSeconds`.
enum Window {
    /// The duration it reports, through the create, the get and the listing.
    Reports(i64),
    /// The member is reported not at all: the source is not one it applies to.
    Absent,
    /// The request is refused, because the window it states is outside the published range.
    Refused,
}

/// One row of the window table: the source the mapping names, what the request states and
/// what the stored mapping must report.
struct WindowRow {
    case: &'static str,
    event_source: Option<&'static str>,
    extras: Value,
    window: Window,
}

/// The rows: the two sources the member applies to with no window stated, with each
/// published bound, with a value between them and with one past each bound, and the sources
/// it does not apply to.
fn window_rows() -> Vec<WindowRow> {
    vec![
        WindowRow {
            case: "a kinesis data stream with no window stated",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({ "StartingPosition": "LATEST" }),
            window: Window::Reports(0),
        },
        WindowRow {
            case: "a dynamodb stream with no window stated",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({ "StartingPosition": "LATEST", "BatchSize": 1 }),
            window: Window::Reports(0),
        },
        WindowRow {
            case: "a dynamodb stream whose window is the lower bound",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({
                "StartingPosition": "TRIM_HORIZON",
                "TumblingWindowInSeconds": WINDOW_MIN,
            }),
            window: Window::Reports(WINDOW_MIN),
        },
        WindowRow {
            case: "a kinesis data stream whose window is the upper bound",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({
                "StartingPosition": "LATEST",
                "TumblingWindowInSeconds": WINDOW_MAX,
            }),
            window: Window::Reports(WINDOW_MAX),
        },
        WindowRow {
            case: "a dynamodb stream whose window is between the bounds",
            event_source: Some(DYNAMODB_SOURCE),
            extras: json!({
                "StartingPosition": "LATEST",
                "TumblingWindowInSeconds": 450,
            }),
            window: Window::Reports(450),
        },
        WindowRow {
            case: "a kinesis data stream whose window is one second past the upper bound",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({
                "StartingPosition": "LATEST",
                "TumblingWindowInSeconds": WINDOW_MAX + 1,
            }),
            window: Window::Refused,
        },
        WindowRow {
            case: "a kinesis data stream whose window is one second below the lower bound",
            event_source: Some(KINESIS_SOURCE),
            extras: json!({
                "StartingPosition": "LATEST",
                "TumblingWindowInSeconds": WINDOW_MIN - 1,
            }),
            window: Window::Refused,
        },
        WindowRow {
            case: "a queue, which no tumbling window applies to",
            event_source: Some(QUEUE_SOURCE),
            extras: json!({}),
            window: Window::Absent,
        },
        WindowRow {
            case: "an msk cluster, which no tumbling window applies to",
            event_source: Some(MSK_SOURCE),
            extras: json!({ "Topics": ["order-events"] }),
            window: Window::Absent,
        },
        WindowRow {
            case: "a documentdb change stream, which no tumbling window applies to",
            event_source: Some(DOCDB_SOURCE),
            // A mapping onto a DocumentDB cluster names the database it consumes inside
            // it, which is what makes the create acceptable at all.
            extras: json!({ "DocumentDBEventSourceConfig": { "DatabaseName": "orders" } }),
            window: Window::Absent,
        },
        WindowRow {
            case: "a self-managed kafka cluster, which no tumbling window applies to",
            event_source: None,
            extras: self_managed_cluster(),
            window: Window::Absent,
        },
    ]
}

/// The `TumblingWindowInSeconds` a reported mapping carries, as JSON — `Value::Null` when the
/// member is absent.
fn reported_window(mapping: &Value) -> Value {
    mapping
        .get("TumblingWindowInSeconds")
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULE: `TumblingWindowInSeconds` is the duration of a processing window for the two shard
/// sources it is documented for, where "a value of 0 seconds indicates no tumbling window" —
/// so a mapping on one of them that stated no window reports 0, one that stated a window
/// inside the published range reports it, and a mapping on any other source reports the member
/// not at all. Every reading goes through the create, the get and the listing, so a window
/// that was answered but not stored fails here.
#[tokio::test]
async fn a_shard_source_mapping_reports_the_tumbling_window_it_was_created_with() {
    let router = emulator();
    for (index, row) in window_rows().iter().enumerate() {
        let case = row.case;
        let function = format!("esm-window-{index}");
        create_function(&router, &function).await;

        let answer = create_mapping(
            &router,
            &create_request(&function, row.event_source, &row.extras),
        )
        .await;

        let expected = match row.window {
            Window::Reports(seconds) => json!(seconds),
            Window::Absent => Value::Null,
            Window::Refused => {
                assert_eq!(
                    answer.status,
                    400,
                    "case {case:?}: a window outside the published range must be refused, got \
                     {} {}",
                    answer.status,
                    answer.text()
                );
                let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
                let held = listed.mappings().len();
                assert_eq!(
                    held, 0,
                    "case {case:?}: a refused create must store no mapping, got {held} \
                     mapping(s) in {}",
                    listed.text()
                );
                continue;
            }
        };

        assert_eq!(
            answer.status,
            202,
            "case {case:?}: the create must be accepted, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            reported_window(&answer.json()),
            expected,
            "case {case:?}: the create must report {expected} for TumblingWindowInSeconds, \
             got {}",
            answer.text()
        );

        let uuid = answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: the create answers a UUID"));
        let fetched = get_mapping(&router, &uuid).await;
        assert_eq!(
            (fetched.status, reported_window(&fetched.json())),
            (200, expected.clone()),
            "case {case:?}: the get must report {expected} for TumblingWindowInSeconds, got \
             {} {}",
            fetched.status,
            fetched.text()
        );

        let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
        let entry = listed
            .mappings()
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the listing holds the mapping"));
        assert_eq!(
            reported_window(&entry),
            expected,
            "case {case:?}: the listing must report {expected} for TumblingWindowInSeconds, \
             got {}",
            listed.text()
        );
    }
}
