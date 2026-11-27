// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's ENHANCED SHARD-LEVEL METRICS, end to end over the wire:
//! `CreateStream` -> `EnableEnhancedMonitoring` / `DisableEnhancedMonitoring` ->
//! `DescribeStreamSummary` / `DescribeStream`.
//!
//! RULE: a stream carries a SET of enhanced shard-level metrics, empty until something
//! enhances one. `EnableEnhancedMonitoring` ADDS the metrics its request names to that set
//! and `DisableEnhancedMonitoring` REMOVES them, the catch-all value standing for every
//! metric the model's `MetricsName` enum lists; each call reports the set as it was before
//! the call (`CurrentShardLevelMetrics`) beside the set it leaves behind
//! (`DesiredShardLevelMetrics`) and persists the latter, so whatever a change stored is
//! what every later read of that stream reports. Naming a metric that is already enhanced,
//! or disabling one nothing enhanced, therefore leaves the same set behind. A value the
//! enum does not accept is refused with a 400 and leaves the stored set exactly as it was;
//! either call resolves its stream by `StreamName`, by `StreamARN` or by both inside the
//! region the request was signed for, is `ResourceNotFoundException` for a stream this
//! account and region do not hold, `InvalidArgumentException` for a request naming no
//! stream at all, and `ResourceInUseException` while the stream is not `ACTIVE`.
//!
//! The recorded flows this was derived from enable metrics on ONE stream and ask one
//! invalid metric of the disabling call; those are two rows of the tables below. The rows
//! differ along what the rule generalises over: which operation is called, how many metrics
//! one call names, whether they are already enhanced, whether the catch-all or individual
//! metrics are named, how many calls one stream takes, the identifier the request resolves
//! the stream by, and the status the stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! What that stored set DOES is the second rule of this suite: a stream's data plane publishes
//! its traffic as `AWS/Kinesis` CloudWatch series — stream-level for every stream with no
//! opt-in, and additionally dimensioned by `ShardId` for exactly the metrics the stream's
//! enhanced monitoring names — so the last case mounts CloudWatch beside Kinesis on one engine
//! and reads those series back where a client reads them.
//!
//! A consumer's LAG is the third rule of this suite, and the one the cases at the end of it
//! are about: every read of a shard publishes how far behind the read left its consumer as the
//! SAME figure the read itself answers with (`GetRecordsOutput$MillisBehindLatest`, and the
//! same member of `SubscribeToShardEvent`: "The number of milliseconds the response is from
//! the tip of the stream, indicating how far behind current time the consumer is. A value of
//! zero indicates that record processing is caught up") — into the stream-level per-API series
//! of the reader that took it, with no opt-in, and under `{StreamName, ShardId}` as well for a
//! stream whose enhanced monitoring names the shard-level spelling of that lag. The figure is
//! positive while records remain ahead of the reader and zero once it has read to the tip, and
//! the zero is published too, so a client alarming on the lag evaluates either way.
//!
//! WHAT a call contributes to those series is the fourth rule, and the one the last cases are
//! about: a call is counted for what it ACTUALLY did, so the incoming-volume series count the
//! records and the record data the shards stored, a record a shard's published write rate
//! turned down is counted into the throughput-exceeded series instead of into the volume, and
//! one `PutRecords` call's outcome split reports which of the entries it carried ended up each
//! way.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    counted, shard_id, stream_not_found_message, STREAM_CREATING,
    STREAM_IDENTIFIER_REQUIRED_MESSAGE, THROUGHPUT_EXCEEDED_ERROR_CODE,
};
use service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::stream_tags::{
    error_code, error_message, past_activation, poll_until, stream_arn, Emulator, REGION,
};

/// Every shard-level metric enhanced monitoring reports, spelled out here rather than read
/// from the service so a catalog that drifts fails this test instead of agreeing with
/// itself: the values of the `MetricsName` enum
/// (`smithy_query(query="shape",shape="MetricsName")`) less the catch-all `ALL`, which
/// "enables every metric" (`EnableEnhancedMonitoringInput$ShardLevelMetrics`) rather than
/// naming one. This is also the order every report of the set is in, so two reads of one
/// stream cannot disagree about it.
const EVERY_METRIC: &[&str] = &[
    "IncomingBytes",
    "IncomingRecords",
    "IteratorAgeMilliseconds",
    "OutgoingBytes",
    "OutgoingRecords",
    "ReadProvisionedThroughputExceeded",
    "WriteProvisionedThroughputExceeded",
];

/// The catch-all value that stands for [`EVERY_METRIC`].
const ALL: &str = "ALL";

/// Which way a step moves the stream's set.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// `EnableEnhancedMonitoring`, which adds to the set.
    Enable,
    /// `DisableEnhancedMonitoring`, which takes away from it.
    Disable,
}

impl Change {
    /// The operation this direction is served by.
    fn operation(self) -> &'static str {
        match self {
            Self::Enable => "EnableEnhancedMonitoring",
            Self::Disable => "DisableEnhancedMonitoring",
        }
    }
}

/// How a step addresses its stream: either identifier resolves it, and so do both together.
#[derive(Debug, Clone, Copy)]
enum By {
    Name,
    Arn,
    Both,
}

impl By {
    /// The request body naming `stream`, carrying `metrics`.
    fn body(self, stream: &str, metrics: &[&str]) -> Value {
        match self {
            Self::Name => json!({ "StreamName": stream, "ShardLevelMetrics": metrics }),
            Self::Arn => json!({
                "StreamARN": stream_arn(REGION, stream),
                "ShardLevelMetrics": metrics,
            }),
            Self::Both => json!({
                "StreamName": stream,
                "StreamARN": stream_arn(REGION, stream),
                "ShardLevelMetrics": metrics,
            }),
        }
    }
}

/// One step of a row: the change it asks for, the metrics it names, how it addresses the
/// stream, and the set the stream must report once it is served.
struct Step {
    change: Change,
    metrics: &'static [&'static str],
    by: By,
    leaves: &'static [&'static str],
}

/// The metrics `expected` names, in the order every report of the set is in.
fn in_catalog_order(expected: &[&str]) -> Vec<String> {
    EVERY_METRIC
        .iter()
        .filter(|metric| expected.contains(metric))
        .map(|metric| (*metric).to_string())
        .collect()
}

/// The `ShardLevelMetrics` list a response member carries, as strings.
fn metric_list(case: &str, response: &Value, member: &str) -> Vec<String> {
    let list = response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: the response must carry {member}, got {response}"));
    list.iter()
        .map(|metric| {
            metric
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: {member} holds strings, got {metric}"))
                .to_string()
        })
        .collect()
}

/// The enhanced set `DescribeStreamSummary` reports for `stream`.
async fn summarised_metrics(emulator: &Emulator, case: &str, stream: &str) -> Vec<String> {
    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": stream }))
        .await;
    let entries = described["StreamDescriptionSummary"]
        .get("EnhancedMonitoring")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: the summary must carry EnhancedMonitoring, got {described}")
        });
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected one EnhancedMonitoring entry, got {described}"
    );
    metric_list(case, &entries[0], "ShardLevelMetrics")
}

/// The enhanced set `DescribeStream` reports for `stream`.
async fn described_metrics(emulator: &Emulator, case: &str, stream: &str) -> Vec<String> {
    let described = emulator
        .ok(case, "DescribeStream", &json!({ "StreamName": stream }))
        .await;
    let entries = described["StreamDescription"]
        .get("EnhancedMonitoring")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: the description must carry EnhancedMonitoring, got {described}")
        });
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected one EnhancedMonitoring entry, got {described}"
    );
    metric_list(case, &entries[0], "ShardLevelMetrics")
}

/// Every accepted change reports the set it replaced beside the set it stored, and the
/// stored one is what both reads of the stream report afterwards.
#[tokio::test]
async fn a_metric_change_is_what_a_read_reports() {
    // Each row is a sequence of changes applied to ONE stream, each with the set the stream
    // must carry afterwards. A freshly created stream enhances no metric, so every row
    // starts from the empty set.
    let rows: Vec<(&str, Vec<Step>)> = vec![
        (
            "one-metric",
            vec![Step {
                change: Change::Enable,
                metrics: &["IncomingBytes"],
                by: By::Name,
                leaves: &["IncomingBytes"],
            }],
        ),
        (
            "several-metrics-at-once",
            vec![Step {
                change: Change::Enable,
                // Named out of the catalog's order: the request's order is not the set's.
                metrics: &["OutgoingRecords", "IncomingRecords"],
                by: By::Arn,
                leaves: &["IncomingRecords", "OutgoingRecords"],
            }],
        ),
        (
            "the-catch-all",
            vec![Step {
                change: Change::Enable,
                metrics: &[ALL],
                by: By::Both,
                leaves: EVERY_METRIC,
            }],
        ),
        (
            "metrics-accumulate",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["IteratorAgeMilliseconds"],
                    by: By::Name,
                    leaves: &["IteratorAgeMilliseconds"],
                },
                Step {
                    change: Change::Enable,
                    metrics: &["OutgoingBytes", "IncomingBytes"],
                    by: By::Arn,
                    leaves: &["IncomingBytes", "IteratorAgeMilliseconds", "OutgoingBytes"],
                },
            ],
        ),
        (
            "enabling-what-is-already-enhanced-changes-nothing",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["ReadProvisionedThroughputExceeded"],
                    by: By::Name,
                    leaves: &["ReadProvisionedThroughputExceeded"],
                },
                Step {
                    change: Change::Enable,
                    // The same metric twice in one request, and one already enhanced.
                    metrics: &[
                        "ReadProvisionedThroughputExceeded",
                        "ReadProvisionedThroughputExceeded",
                    ],
                    by: By::Name,
                    leaves: &["ReadProvisionedThroughputExceeded"],
                },
            ],
        ),
        (
            "one-metric-back-off-the-catch-all",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &[ALL],
                    by: By::Name,
                    leaves: EVERY_METRIC,
                },
                Step {
                    change: Change::Disable,
                    metrics: &["IncomingBytes"],
                    by: By::Arn,
                    leaves: &[
                        "IncomingRecords",
                        "IteratorAgeMilliseconds",
                        "OutgoingBytes",
                        "OutgoingRecords",
                        "ReadProvisionedThroughputExceeded",
                        "WriteProvisionedThroughputExceeded",
                    ],
                },
            ],
        ),
        (
            "enable-then-disable-the-same-metrics",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["WriteProvisionedThroughputExceeded", "OutgoingBytes"],
                    by: By::Name,
                    leaves: &["OutgoingBytes", "WriteProvisionedThroughputExceeded"],
                },
                Step {
                    change: Change::Disable,
                    metrics: &["OutgoingBytes", "WriteProvisionedThroughputExceeded"],
                    by: By::Name,
                    leaves: &[],
                },
            ],
        ),
        (
            "disabling-what-nothing-enhanced-changes-nothing",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["IncomingRecords"],
                    by: By::Name,
                    leaves: &["IncomingRecords"],
                },
                Step {
                    change: Change::Disable,
                    metrics: &["OutgoingRecords"],
                    by: By::Name,
                    leaves: &["IncomingRecords"],
                },
            ],
        ),
        (
            "the-catch-all-takes-every-metric-off",
            vec![
                Step {
                    change: Change::Enable,
                    metrics: &["IncomingBytes", "OutgoingBytes", "IteratorAgeMilliseconds"],
                    by: By::Name,
                    leaves: &["IncomingBytes", "IteratorAgeMilliseconds", "OutgoingBytes"],
                },
                Step {
                    change: Change::Disable,
                    metrics: &[ALL],
                    by: By::Both,
                    leaves: &[],
                },
            ],
        ),
        (
            "disabling-nothing-enhanced-at-all",
            vec![Step {
                change: Change::Disable,
                metrics: &["IncomingBytes"],
                by: By::Name,
                leaves: &[],
            }],
        ),
    ];

    let emulator = Emulator::new();
    for (name, _) in &rows {
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for (name, steps) in &rows {
        let mut carried: Vec<String> = Vec::new();
        assert_eq!(
            summarised_metrics(&emulator, name, name).await,
            carried,
            "case {name}: expected a new stream to enhance no metric"
        );

        for (index, step) in steps.iter().enumerate() {
            let case = format!("{name}-{index}-{}", step.change.operation());
            let response = emulator
                .ok(
                    &case,
                    step.change.operation(),
                    &step.by.body(name, step.metrics),
                )
                .await;

            // "The name of the Kinesis data stream" and "the ARN of the stream" are the
            // stream the call resolved, whichever identifier named it.
            assert_eq!(
                response.get("StreamName").and_then(Value::as_str),
                Some(*name),
                "case {case}: expected the response to name the stream, got {response}"
            );
            assert_eq!(
                response.get("StreamARN").and_then(Value::as_str),
                Some(stream_arn(REGION, name).as_str()),
                "case {case}: expected the response to carry the stream's ARN, got {response}"
            );

            let current = metric_list(&case, &response, "CurrentShardLevelMetrics");
            assert_eq!(
                current, carried,
                "case {case}: expected the set before the call, got {current:?}"
            );
            let desired = metric_list(&case, &response, "DesiredShardLevelMetrics");
            let expected = in_catalog_order(step.leaves);
            assert_eq!(
                desired, expected,
                "case {case}: expected the call to leave {expected:?}, got {desired:?}"
            );

            // The stored set is what both reads of the stream report.
            let summarised = summarised_metrics(&emulator, &case, name).await;
            assert_eq!(
                summarised, expected,
                "case {case}: expected the summary to report {expected:?}, got {summarised:?}"
            );
            let described = described_metrics(&emulator, &case, name).await;
            assert_eq!(
                described, expected,
                "case {case}: expected the description to report {expected:?}, got {described:?}"
            );
            carried = expected;
        }
    }
}

/// One refused change: the metrics it names, the stream it addresses and the refusal it
/// must answer.
struct Refusal {
    case: &'static str,
    change: Change,
    metrics: &'static [&'static str],
    code: &'static str,
    /// What the refusal's message must name, built from the row's own stream and metrics.
    says: fn(&str, &[&str]) -> Vec<String>,
    /// Whether the row's stream exists and is `ACTIVE` when the call arrives.
    subject: Subject,
}

/// What the stream a refusal row addresses looks like when the call arrives.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// An `ACTIVE` stream carrying one enhanced metric, so a refusal that wrote anything
    /// would show.
    Enhanced,
    /// A stream whose create has not fallen due yet.
    Creating,
    /// No stream at all.
    Absent,
}

/// The metrics the `Enhanced` subject carries when the call arrives.
const CARRIED: &[&str] = &["IncomingBytes"];

/// A refused change answers a client error and leaves the stored set exactly as it was.
#[tokio::test]
async fn a_refused_metric_change_leaves_the_set_as_it_was() {
    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "unknown-metric",
            change: Change::Disable,
            metrics: &["BytesIncoming"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            case: "unknown-metric-enabling",
            change: Change::Enable,
            metrics: &["BytesIncoming"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            // The enum's values are case-sensitive, so a metric spelled another way names
            // no metric.
            case: "a-metric-in-another-casing",
            change: Change::Enable,
            metrics: &["incomingbytes"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            case: "the-catch-all-in-another-casing",
            change: Change::Enable,
            metrics: &["all"],
            code: "ValidationException",
            says: |_, metrics| refused_value(metrics),
            subject: Subject::Enhanced,
        },
        Refusal {
            // One good metric does not carry a bad one through: the whole list is refused
            // and nothing is enhanced.
            case: "one-good-metric-beside-a-bad-one",
            change: Change::Enable,
            metrics: &["OutgoingBytes", "SomethingElse"],
            code: "ValidationException",
            says: |_, _| refused_value(&["SomethingElse"]),
            subject: Subject::Enhanced,
        },
        Refusal {
            case: "no-such-stream",
            change: Change::Enable,
            metrics: &["IncomingBytes"],
            code: "ResourceNotFoundException",
            says: |stream, _| vec![stream_not_found_message(stream)],
            subject: Subject::Absent,
        },
        Refusal {
            case: "no-such-stream-disabling",
            change: Change::Disable,
            metrics: &[ALL],
            code: "ResourceNotFoundException",
            says: |stream, _| vec![stream_not_found_message(stream)],
            subject: Subject::Absent,
        },
        Refusal {
            case: "still-creating",
            change: Change::Enable,
            metrics: &["IncomingBytes"],
            code: "ResourceInUseException",
            says: |_, _| vec![STREAM_CREATING.to_string()],
            subject: Subject::Creating,
        },
    ];

    let emulator = Emulator::new();
    for row in &rows {
        if let Subject::Enhanced = row.subject {
            emulator
                .ok(
                    row.case,
                    "CreateStream",
                    &json!({ "StreamName": row.case, "ShardCount": 1 }),
                )
                .await;
        }
    }
    tokio::time::sleep(past_activation()).await;

    for row in &rows {
        let case = row.case;
        match row.subject {
            Subject::Enhanced => {
                emulator
                    .ok(
                        case,
                        "EnableEnhancedMonitoring",
                        &json!({ "StreamName": case, "ShardLevelMetrics": CARRIED }),
                    )
                    .await;
            }
            Subject::Creating => {
                emulator
                    .ok(
                        case,
                        "CreateStream",
                        &json!({ "StreamName": case, "ShardCount": 1 }),
                    )
                    .await;
            }
            Subject::Absent => {}
        }

        let (status, response) = emulator
            .call(
                row.change.operation(),
                &json!({ "StreamName": case, "ShardLevelMetrics": row.metrics }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {case}: expected {}, got {response}",
            row.code
        );
        let message = error_message(&response);
        for says in (row.says)(case, row.metrics) {
            assert!(
                message.contains(&says),
                "case {case}: expected the message to name {says:?}, got {message:?}"
            );
        }

        if let Subject::Enhanced = row.subject {
            let after = summarised_metrics(&emulator, case, case).await;
            assert_eq!(
                after,
                in_catalog_order(CARRIED),
                "case {case}: a refused change must leave the stored set untouched, got \
                 {after:?}"
            );
        }
    }
}

/// What a refusal of a value outside the enum must name: the count of refused values, and
/// each value at fault, taken from the request.
fn refused_value(metrics: &[&str]) -> Vec<String> {
    let mut says = vec!["1 validation error detected".to_string()];
    says.extend(metrics.iter().map(|metric| (*metric).to_string()));
    says.push("shardLevelMetrics".to_string());
    says
}

/// A refusal of a value outside the enum names the value at fault, the member that carried
/// it and every value the enum accepts — the three variable parts of the sentence the
/// service states such a refusal with, taken from the request and from the model.
///
/// The sentence itself is the framework's, stated once before any handler runs for every
/// modelled constraint of every operation; what this pins is that the refusal of this
/// member says which value was refused, where it sat and what it had to be.
#[tokio::test]
async fn an_unsupported_metric_is_refused_with_what_the_enum_accepts() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "wording",
            "CreateStream",
            &json!({ "StreamName": "wording", "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    for operation in ["EnableEnhancedMonitoring", "DisableEnhancedMonitoring"] {
        let (status, response) = emulator
            .call(
                operation,
                &json!({
                    "StreamName": "wording",
                    "ShardLevelMetrics": ["IncomingBytes", "NotAMetric"],
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {operation}: expected the refusal, got {status} with body {response}"
        );
        let message = error_message(&response);
        for says in ["1 validation error detected", "NotAMetric", "shardLevelMetrics"] {
            assert!(
                message.contains(says),
                "case {operation}: expected the message to name {says:?}, got {message:?}"
            );
        }
        for metric in EVERY_METRIC.iter().chain(std::iter::once(&ALL)) {
            assert!(
                message.contains(metric),
                "case {operation}: expected the message to name the accepted value \
                 {metric:?}, got {message:?}"
            );
        }
    }
}

/// A request naming no stream at all is refused by the rule every stream operation applies,
/// before any metric is read or written.
#[tokio::test]
async fn a_metric_change_needs_an_identifier() {
    let emulator = Emulator::new();
    for operation in ["EnableEnhancedMonitoring", "DisableEnhancedMonitoring"] {
        let (status, response) = emulator
            .call(operation, &json!({ "ShardLevelMetrics": ["IncomingBytes"] }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {operation}: expected InvalidArgumentException, got {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains(STREAM_IDENTIFIER_REQUIRED_MESSAGE),
            "case {operation}: expected the message to name the identifier rule, got \
             {message:?}"
        );
    }
}

/// A router carrying this service AND CloudWatch over ONE shared engine: the same assembly
/// `core/router` performs, with the two services a published metric involves — the stream's
/// data plane writes the series, and CloudWatch's own store is where a client reads them.
struct Monitored {
    router: MultiServiceRouter,
}

impl Monitored {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here, so requests are unauthenticated.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// Send one request to the service `signing_name` signs for, named under `target`.
    async fn call(&self, signing_name: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
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

    /// One Kinesis operation that must succeed, returning its parsed body.
    async fn kinesis(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call("kinesis", &format!("Kinesis_20131202.{operation}"), body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to be served, got {status} with {parsed}"
        );
        parsed
    }

    /// One CloudWatch operation that must succeed, returning its parsed body.
    async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call(
                "monitoring",
                &format!("GraniteServiceVersion20100801.{operation}"),
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected CloudWatch's {operation} to be served, got {status} with \
             {parsed}"
        );
        parsed
    }
}

/// The namespace Amazon Kinesis Data Streams publishes a stream's own metrics under
/// (<https://docs.aws.amazon.com/streams/latest/dev/monitoring-with-cloudwatch.html>).
const KINESIS_NAMESPACE: &str = "AWS/Kinesis";

/// The dimension that names the stream a series belongs to.
const STREAM_NAME_DIMENSION: &str = "StreamName";

/// The dimension a shard-level series carries beside it.
const SHARD_ID_DIMENSION: &str = "ShardId";

/// What one series holds over a window wide enough for everything a case published.
#[derive(Debug)]
struct Reading {
    /// How many datapoints the window reported.
    datapoints: usize,
    /// The sum of their `Sum` statistics: what the calls contributed.
    sum: f64,
    /// The sum of their `SampleCount` statistics: how many calls contributed.
    sample_count: f64,
    /// The largest figure any one observation in the window carried, or `None` when the
    /// window reported no datapoint at all.
    maximum: Option<f64>,
    /// The unit each datapoint carries.
    units: Vec<String>,
}

/// The largest difference two of these figures may show and still be the same figure: both
/// sides are sums of whole records and whole bytes, so anything above rounding is a real
/// divergence.
const TOLERANCE: f64 = 1e-9;

/// Read one `AWS/Kinesis` series back through CloudWatch, as a client reads it.
async fn series(monitored: &Monitored, case: &str, metric: &str, dimensions: Value) -> Reading {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs_f64();
    let answered = monitored
        .cloudwatch(
            case,
            "GetMetricStatistics",
            &json!({
                "Namespace": KINESIS_NAMESPACE,
                "MetricName": metric,
                "Dimensions": dimensions,
                // A window far wider than any case needs on either side, so nothing here
                // depends on how fast the host runs.
                "StartTime": now - 3_600.0,
                "EndTime": now + 3_600.0,
                "Period": 60,
                "Statistics": ["Sum", "SampleCount", "Maximum"],
            }),
        )
        .await;
    let points: Vec<Value> = answered["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Reading {
        datapoints: points.len(),
        sum: points.iter().filter_map(|point| point["Sum"].as_f64()).sum(),
        sample_count: points
            .iter()
            .filter_map(|point| point["SampleCount"].as_f64())
            .sum(),
        maximum: points
            .iter()
            .filter_map(|point| point["Maximum"].as_f64())
            .reduce(f64::max),
        units: points
            .iter()
            .filter_map(|point| point["Unit"].as_str().map(str::to_string))
            .collect(),
    }
}

/// The dimension set a stream-level series is published under.
fn stream_level(stream: &str) -> Value {
    json!([{ "Name": STREAM_NAME_DIMENSION, "Value": stream }])
}

/// The dimension set a shard-level series is published under.
fn shard_level(stream: &str, shard: &str) -> Value {
    json!([
        { "Name": STREAM_NAME_DIMENSION, "Value": stream },
        { "Name": SHARD_ID_DIMENSION, "Value": shard },
    ])
}

/// The record data every write of the metrics cases carries, so a byte sum is a record count
/// times one record's size.
const PAYLOAD: &str = "metrics-payload";

/// The partition key every one of those writes carries, which counts towards the record's size
/// beside its data: "when the data blob (the payload before base64-encoding) is added to the
/// partition key size, the total size must not exceed the maximum record size"
/// (`PutRecordInput$Data`).
const PARTITION_KEY: &str = "metrics";

/// One `PutRecord` of [`PAYLOAD`] into `stream`.
async fn put_one(monitored: &Monitored, case: &str, stream: &str) {
    monitored
        .kinesis(
            case,
            "PutRecord",
            &json!({
                "StreamName": stream,
                "PartitionKey": PARTITION_KEY,
                "Data": aws_smithy_types::base64::encode(PAYLOAD),
            }),
        )
        .await;
}

/// Create `stream` with one shard and wait for the ACTIVE its create owes it.
async fn active_stream(monitored: &Monitored, case: &str, stream: &str) {
    active_stream_sized(monitored, case, stream, None).await;
}

/// Create `stream` with one shard, configured for records of `max_record_kib` where a case
/// asks for a record size of its own, and wait for the ACTIVE its create owes it.
async fn active_stream_sized(
    monitored: &Monitored,
    case: &str,
    stream: &str,
    max_record_kib: Option<i64>,
) {
    let mut body = json!({ "StreamName": stream, "ShardCount": 1 });
    if let Some(kib) = max_record_kib {
        body["MaxRecordSizeInKiB"] = json!(kib);
    }
    monitored.kinesis(case, "CreateStream", &body).await;
    poll_until(case, &format!("the stream {stream} is active"), || async {
        let (status, response) = monitored
            .call(
                "kinesis",
                "Kinesis_20131202.DescribeStreamSummary",
                &json!({ "StreamName": stream }),
            )
            .await;
        status == 200
            && response["StreamDescriptionSummary"]["StreamStatus"].as_str() == Some("ACTIVE")
    })
    .await;
}

/// RULE: a stream's data plane publishes its traffic as `AWS/Kinesis` series — stream-level
/// for every stream with no opt-in, and additionally dimensioned by `ShardId` for exactly the
/// metrics the stream's own enhanced monitoring names. The sums are what the calls did: N
/// writes of equal-length payloads come to `IncomingRecords` N and `IncomingBytes` N times
/// that length, a read comes to `OutgoingRecords` for what it served, and a stream nothing
/// enhanced has no shard-dimensioned series at all.
///
/// The cases span what the rule generalises over: both write APIs (`PutRecord` and the
/// `PutRecords` batch) and the read, a stream with enhanced monitoring and a twin without one,
/// metrics inside the enabled set and metrics outside it, and the window before the enable
/// beside the window after it.
#[tokio::test]
async fn a_streams_data_plane_publishes_its_aws_kinesis_series() {
    let monitored = Monitored::new();
    let armed = "metrics-armed";
    let bare = "metrics-bare";
    let shard = shard_id(0);
    let record_bytes = f64::from(
        u32::try_from(PAYLOAD.len() + PARTITION_KEY.len())
            .expect("the records of these cases are a few bytes long"),
    );

    let case = "before the enable";
    active_stream(&monitored, case, armed).await;
    active_stream(&monitored, case, bare).await;
    for _ in 0..4 {
        put_one(&monitored, case, armed).await;
    }
    for _ in 0..2 {
        put_one(&monitored, case, bare).await;
    }

    // Every stream publishes the stream-level series with no opt-in at all...
    for (stream, records) in [(armed, 4.0), (bare, 2.0)] {
        let reading = series(
            &monitored,
            case,
            "IncomingRecords",
            stream_level(stream),
        )
        .await;
        assert!(
            (reading.sum - records).abs() < TOLERANCE,
            "case {case}: expected {stream}'s stream-level IncomingRecords to sum to \
             {records}, got {reading:?}"
        );
        assert!(
            (reading.sample_count - records).abs() < TOLERANCE,
            "case {case}: expected one observation per write of {stream}, got {reading:?}"
        );
        assert!(
            reading.units.iter().all(|unit| unit == "Count"),
            "case {case}: expected a record tally in Count, got {reading:?}"
        );
        let bytes = series(&monitored, case, "IncomingBytes", stream_level(stream)).await;
        assert!(
            (bytes.sum - records * record_bytes).abs() < TOLERANCE,
            "case {case}: expected {stream}'s stream-level IncomingBytes to sum to the \
             records' own data, got {bytes:?}"
        );
        assert!(
            bytes.units.iter().all(|unit| unit == "Bytes"),
            "case {case}: expected a data measurement in Bytes, got {bytes:?}"
        );
    }

    // ... and NO shard-dimensioned series until enhanced monitoring names the metric.
    for stream in [armed, bare] {
        for metric in ["IncomingRecords", "IncomingBytes", "OutgoingRecords"] {
            let reading = series(&monitored, case, metric, shard_level(stream, &shard)).await;
            assert_eq!(
                reading.datapoints, 0,
                "case {case}: a stream whose enhanced monitoring names nothing publishes no \
                 ShardId-dimensioned {metric} for {stream}, got {reading:?}"
            );
        }
    }

    // Arm ONE of the two streams, and write to both again: three single writes and one batch
    // of two, so both write APIs contribute.
    let case = "after the enable";
    monitored
        .kinesis(
            case,
            "EnableEnhancedMonitoring",
            &json!({
                "StreamName": armed,
                "ShardLevelMetrics": ["IncomingRecords", "IncomingBytes", "OutgoingRecords"],
            }),
        )
        .await;
    for _ in 0..3 {
        put_one(&monitored, case, armed).await;
    }
    monitored
        .kinesis(
            case,
            "PutRecords",
            &json!({
                "StreamName": armed,
                "Records": [
                    {
                        "PartitionKey": PARTITION_KEY,
                        "Data": aws_smithy_types::base64::encode(PAYLOAD),
                    },
                    {
                        "PartitionKey": PARTITION_KEY,
                        "Data": aws_smithy_types::base64::encode(PAYLOAD),
                    },
                ],
            }),
        )
        .await;
    put_one(&monitored, case, bare).await;

    // The stream-level series keep counting every write, before and after the enable.
    for (stream, records) in [(armed, 9.0), (bare, 3.0)] {
        let reading = series(&monitored, case, "IncomingRecords", stream_level(stream)).await;
        assert!(
            (reading.sum - records).abs() < TOLERANCE,
            "case {case}: expected {stream}'s stream-level IncomingRecords to sum to \
             {records}, got {reading:?}"
        );
    }

    // The armed stream's shard-level series carry what was written AFTER the enable, under
    // the SAME metric names, one observation per call.
    for (metric, expected) in [
        ("IncomingRecords", 5.0),
        ("IncomingBytes", 5.0 * record_bytes),
    ] {
        let reading = series(&monitored, case, metric, shard_level(armed, &shard)).await;
        assert!(
            (reading.sum - expected).abs() < TOLERANCE,
            "case {case}: expected the ShardId-dimensioned {metric} of {armed} to sum to \
             {expected}, got {reading:?}"
        );
        assert!(
            (reading.sample_count - 4.0).abs() < TOLERANCE,
            "case {case}: expected one observation per call that wrote to the shard (three \
             PutRecord and one PutRecords), got {reading:?}"
        );
    }

    // A metric the enable did NOT name has no shard-dimensioned series, and the twin stream
    // has none at all.
    for (stream, metric) in [
        (armed, "OutgoingBytes"),
        (armed, "IteratorAgeMilliseconds"),
        (bare, "IncomingRecords"),
        (bare, "IncomingBytes"),
    ] {
        let reading = series(&monitored, case, metric, shard_level(stream, &shard)).await;
        assert_eq!(
            reading.datapoints, 0,
            "case {case}: expected no ShardId-dimensioned {metric} for {stream}, got \
             {reading:?}"
        );
    }

    // The per-API families of the calls themselves, which carry no shard dimension.
    for (metric, expected) in [
        ("PutRecord.Success", 7.0),
        ("PutRecords.SuccessfulRecords", 2.0),
        ("PutRecords.TotalRecords", 2.0),
    ] {
        let reading = series(&monitored, case, metric, stream_level(armed)).await;
        assert!(
            (reading.sum - expected).abs() < TOLERANCE,
            "case {case}: expected {metric} of {armed} to sum to {expected}, got {reading:?}"
        );
    }

    // A series exists where its own kind of traffic happened and nowhere else: the retrieval
    // counters have had no read yet, and the shard-dimensioned form of a counter the enable
    // did not name stays absent whatever the stream's calls do.
    for (metric, dimensions) in [
        ("OutgoingRecords", stream_level(armed)),
        ("OutgoingRecords", shard_level(armed, &shard)),
        ("WriteProvisionedThroughputExceeded", stream_level(armed)),
        ("WriteProvisionedThroughputExceeded", shard_level(armed, &shard)),
        ("OutgoingBytes", shard_level(armed, &shard)),
    ] {
        let reading = series(&monitored, case, metric, dimensions.clone()).await;
        assert_eq!(
            reading.datapoints, 0,
            "case {case}: expected no {metric} under {dimensions} while nothing of that kind \
             has happened, got {reading:?}"
        );
    }

    // A read of everything the armed stream holds publishes what it served, stream-level and
    // \u2014 since the enable named it \u2014 per shard.
    let case = "after the read";
    let iterator = monitored
        .kinesis(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": armed,
                "ShardId": shard,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetShardIterator reported no iterator"))
        .to_string();
    let page = monitored
        .kinesis(case, "GetRecords", &json!({ "ShardIterator": iterator }))
        .await;
    let served = page["Records"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        served, 9,
        "case {case}: expected the read to serve every record written, got {page}"
    );

    for (metric, dimensions, expected) in [
        ("OutgoingRecords", stream_level(armed), 9.0),
        ("OutgoingRecords", shard_level(armed, &shard), 9.0),
        ("OutgoingBytes", stream_level(armed), 9.0 * record_bytes),
        ("GetRecords.Records", stream_level(armed), 9.0),
        ("GetRecords.Bytes", stream_level(armed), 9.0 * record_bytes),
        ("GetRecords.Success", stream_level(armed), 1.0),
    ] {
        let reading = series(&monitored, case, metric, dimensions).await;
        assert!(
            (reading.sum - expected).abs() < TOLERANCE,
            "case {case}: expected {metric} to sum to {expected} after the read, got \
             {reading:?}"
        );
    }

    // The PUSH reader retrieves records from the same shard, so it counts into the same
    // retrieval series beside its own per-API family: the rule is about records leaving the
    // shard, not about which reader asked for them.
    let case = "after the subscription";
    let consumer = monitored
        .kinesis(
            case,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn(REGION, armed), "ConsumerName": "metrics-reader" }),
        )
        .await["Consumer"]["ConsumerARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a registration answers a ConsumerARN"))
        .to_string();
    poll_until(case, "the consumer reports ACTIVE", || async {
        let (status, response) = monitored
            .call(
                "kinesis",
                "Kinesis_20131202.DescribeStreamConsumer",
                &json!({ "ConsumerARN": consumer }),
            )
            .await;
        status == 200
            && response["ConsumerDescription"]["ConsumerStatus"].as_str() == Some("ACTIVE")
    })
    .await;
    let (status, _) = monitored
        .call(
            "kinesis",
            "Kinesis_20131202.SubscribeToShard",
            &json!({
                "ConsumerARN": consumer,
                "ShardId": shard,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected the subscription to be established, got {status}"
    );

    for (metric, dimensions, expected) in [
        // The nine records left the shard twice: once to the pull reader, once to this
        // subscription.
        ("OutgoingRecords", stream_level(armed), 18.0),
        ("OutgoingRecords", shard_level(armed, &shard), 18.0),
        ("SubscribeToShardEvent.Records", stream_level(armed), 9.0),
        (
            "SubscribeToShardEvent.Bytes",
            stream_level(armed),
            9.0 * record_bytes,
        ),
        ("SubscribeToShard.Success", stream_level(armed), 1.0),
    ] {
        let reading = series(&monitored, case, metric, dimensions).await;
        assert!(
            (reading.sum - expected).abs() < TOLERANCE,
            "case {case}: expected {metric} to sum to {expected} after the subscription, got \
             {reading:?}"
        );
    }
}

/// The stream-level series the PULL reader publishes its lag into: a per-API family, whose
/// name carries the operation it is about, which is why the shard-level catalog
/// ([`EVERY_METRIC`]) does not list this spelling
/// (<https://docs.aws.amazon.com/streams/latest/dev/monitoring-with-cloudwatch.html>).
const PULL_LAG: &str = "GetRecords.IteratorAgeMilliseconds";

/// The stream-level series the PUSH reader publishes its lag into: its own per-API family,
/// named after the member its event reports the same figure in.
const PUSH_LAG: &str = "SubscribeToShardEvent.MillisBehindLatest";

/// The SHARD-dimensioned spelling of that same lag: the value [`EVERY_METRIC`] carries for it,
/// i.e. a member of the `MetricsName` catalog enhanced monitoring enables, which is why the
/// per-shard series of the lag is published under this name rather than under a per-API one.
const SHARD_LAG: &str = "IteratorAgeMilliseconds";

/// Another metric of that same catalog, for the case that arms a stream with a shard-level
/// metric which is NOT the lag.
const SHARD_OUTGOING_RECORDS: &str = "OutgoingRecords";

/// The unit a lag is published in: the `StandardUnit` value for milliseconds.
const MILLISECONDS: &str = "Milliseconds";

/// A lag in milliseconds as a datapoint carries it. Every lag these cases measure is a few
/// milliseconds of a test's own run, so the conversion is exact.
fn millis(lag: i64) -> f64 {
    f64::from(i32::try_from(lag).expect("a lag of these cases is a few milliseconds"))
}

/// A `TRIM_HORIZON` iterator over `shard` of `stream`: the position of "the oldest data record
/// in the shard" (`GetShardIteratorInput$ShardIteratorType`), where a consumer that has read
/// nothing yet starts.
async fn horizon_iterator(monitored: &Monitored, case: &str, stream: &str, shard: &str) -> String {
    monitored
        .kinesis(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetShardIterator reports an iterator"))
        .to_string()
}

/// Read at most `limit` records from the oldest record of `shard`, answering how many records
/// the page served and the lag it reported.
async fn read_from_the_horizon(
    monitored: &Monitored,
    case: &str,
    stream: &str,
    shard: &str,
    limit: i64,
) -> (usize, i64) {
    let iterator = horizon_iterator(monitored, case, stream, shard).await;
    let page = monitored
        .kinesis(
            case,
            "GetRecords",
            &json!({ "ShardIterator": iterator, "Limit": limit }),
        )
        .await;
    let served = page["Records"].as_array().map(Vec::len).unwrap_or_default();
    let lag = page["MillisBehindLatest"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: GetRecords reports MillisBehindLatest, got {page}"));
    (served, lag)
}

/// The series `metric` must hold `reported` — the figure the read itself answered with — as the
/// largest lag any of its observations carried, in milliseconds.
fn carries_the_lag(case: &str, metric: &str, reading: &Reading, reported: i64) {
    assert!(
        reading.datapoints >= 1,
        "case {case}: expected {metric} to carry the lag the read measured, got {reading:?}"
    );
    assert!(
        reading
            .maximum
            .is_some_and(|peak| (peak - millis(reported)).abs() < TOLERANCE),
        "case {case}: expected {metric} to carry the lag the read reported ({reported} ms) as \
         its largest observation, got {reading:?}"
    );
    assert!(
        reading.units.iter().all(|unit| unit == MILLISECONDS),
        "case {case}: expected {metric} to be published in {MILLISECONDS}, got {reading:?}"
    );
}

/// RULE, for the pull reader: one `GetRecords` publishes the lag it reported — positive while
/// records remain ahead of the consumer, zero once it has read to the tip — into the
/// stream-level `GetRecords.IteratorAgeMilliseconds` and, because each row's stream enhances
/// the lag, into the shard-dimensioned `IteratorAgeMilliseconds` beside it. The two identities
/// carry the same figure, and neither name exists under the other's dimension set.
///
/// The rows differ along what the rule generalises over: how many records the shard holds, how
/// many of them one page admits, and so whether the read leaves its consumer behind the tip or
/// caught up with it.
#[tokio::test]
async fn a_read_publishes_the_lag_it_reported_into_the_streams_lag_series() {
    assert!(
        EVERY_METRIC.contains(&SHARD_LAG),
        "the shard-level lag is a member of the enhanced-monitoring catalog, which is what a \
         stream can enable it through"
    );
    let shard = shard_id(0);
    // (the case, the records written, the records one page admits, the records it serves,
    // whether the read leaves the consumer behind the tip)
    let rows = [
        ("one record of four", 4, 1, 1, true),
        ("three records of four", 4, 3, 3, true),
        ("every record of four", 4, 10, 4, false),
        ("the only record there is", 1, 1, 1, false),
        ("a shard nothing was written to", 0, 10, 0, false),
    ];

    for (case, written, limit, expect_served, behind) in rows {
        // One engine per row, so a row's series hold only that row's own observations and the
        // largest one of them is the lag its last read reported.
        let monitored = Monitored::new();
        let stream = "lag-series";
        active_stream(&monitored, case, stream).await;
        monitored
            .kinesis(
                case,
                "EnableEnhancedMonitoring",
                &json!({ "StreamName": stream, "ShardLevelMetrics": [SHARD_LAG] }),
            )
            .await;
        for _ in 0..written {
            put_one(&monitored, case, stream).await;
        }

        let reported = if behind {
            // The data a consumer left behind the tip holds ages with the clock, so the case
            // LOOKS again until a read reports that age rather than waiting for a duration.
            // Every look reads the same records from the same position, so the figure it
            // reports only grows and the last one is the largest the series holds.
            let lag = std::cell::Cell::new(0_i64);
            poll_until(
                case,
                "a read short of the tip reports a lag of its own",
                || async {
                    let (served, reported) =
                        read_from_the_horizon(&monitored, case, stream, &shard, limit).await;
                    assert_eq!(
                        served, expect_served,
                        "case {case}: expected the page to serve {expect_served} records, got \
                         {served}"
                    );
                    lag.set(reported);
                    reported > 0
                },
            )
            .await;
            lag.get()
        } else {
            let (served, reported) =
                read_from_the_horizon(&monitored, case, stream, &shard, limit).await;
            assert_eq!(
                served, expect_served,
                "case {case}: expected the page to serve {expect_served} records, got {served}"
            );
            assert_eq!(
                reported, 0,
                "case {case}: a reader that has read to the tip is caught up, so its page \
                 reports no lag"
            );
            reported
        };
        assert_eq!(
            reported > 0,
            behind,
            "case {case}: expected a read left behind the tip to report a lag and a caught-up \
             one to report none, got {reported} ms"
        );

        let at_stream = series(&monitored, case, PULL_LAG, stream_level(stream)).await;
        carries_the_lag(case, PULL_LAG, &at_stream, reported);
        let at_shard = series(&monitored, case, SHARD_LAG, shard_level(stream, &shard)).await;
        carries_the_lag(case, SHARD_LAG, &at_shard, reported);

        // Each identity carries its OWN name: a per-API family is about the call and has no
        // shard dimension, while the lag the enhanced-monitoring catalog names exists only per
        // shard.
        for (metric, dimensions, why) in [
            (
                PULL_LAG,
                shard_level(stream, &shard),
                "a per-API family carries no shard dimension",
            ),
            (
                SHARD_LAG,
                stream_level(stream),
                "the catalog's lag is a shard-level metric",
            ),
        ] {
            let reading = series(&monitored, case, metric, dimensions).await;
            assert_eq!(
                reading.datapoints, 0,
                "case {case}: expected no {metric} under that dimension set, because {why}, got \
                 {reading:?}"
            );
        }
    }
}

/// RULE, for the two identities of the lag: the stream-level reading is published for every
/// stream with no opt-in at all, while the shard-dimensioned one exists only for a stream whose
/// own enhanced monitoring names THAT metric — the rows being a stream that enhanced the lag, a
/// stream that enhanced nothing, and a stream that enhanced another metric of the catalog
/// instead.
#[tokio::test]
async fn the_shard_dimensioned_lag_exists_only_where_enhanced_monitoring_names_it() {
    let monitored = Monitored::new();
    let shard = shard_id(0);
    let case = "three streams, one enhanced set each";
    // (the stream, the shard-level metrics its enhanced monitoring names)
    let rows: [(&str, &[&str]); 3] = [
        ("lag-armed", &[SHARD_LAG]),
        ("lag-bare", &[]),
        ("lag-other-metric", &[SHARD_OUTGOING_RECORDS]),
    ];

    for (stream, metrics) in rows {
        active_stream(&monitored, case, stream).await;
        if !metrics.is_empty() {
            monitored
                .kinesis(
                    case,
                    "EnableEnhancedMonitoring",
                    &json!({ "StreamName": stream, "ShardLevelMetrics": metrics }),
                )
                .await;
        }
        put_one(&monitored, case, stream).await;
        put_one(&monitored, case, stream).await;
        let (served, reported) = read_from_the_horizon(&monitored, case, stream, &shard, 10).await;
        assert_eq!(
            served, 2,
            "case {case}: expected {stream}'s read to serve both records, got {served}"
        );
        assert_eq!(
            reported, 0,
            "case {case}: expected {stream}'s reader to be caught up after reading to the tip, \
             got {reported} ms"
        );

        // The stream-level reading is published whatever the stream enhanced...
        let at_stream = series(&monitored, case, PULL_LAG, stream_level(stream)).await;
        carries_the_lag(case, PULL_LAG, &at_stream, reported);

        // ... and the shard-dimensioned one exactly when the set names the lag itself.
        let at_shard = series(&monitored, case, SHARD_LAG, shard_level(stream, &shard)).await;
        if metrics.contains(&SHARD_LAG) {
            carries_the_lag(case, SHARD_LAG, &at_shard, reported);
        } else {
            assert_eq!(
                at_shard.datapoints, 0,
                "case {case}: {stream} does not enhance {SHARD_LAG}, so it publishes no \
                 ShardId-dimensioned series of it, got {at_shard:?}"
            );
        }

        // A stream that enhanced a DIFFERENT metric still publishes that one per shard, so the
        // absence above is about which metric the set names and nothing else.
        let other = series(
            &monitored,
            case,
            SHARD_OUTGOING_RECORDS,
            shard_level(stream, &shard),
        )
        .await;
        let expected_other = usize::from(metrics.contains(&SHARD_OUTGOING_RECORDS));
        assert_eq!(
            other.datapoints.min(1),
            expected_other,
            "case {case}: expected the ShardId-dimensioned {SHARD_OUTGOING_RECORDS} of {stream} \
             to exist only where its set names it, got {other:?}"
        );
        if expected_other == 1 {
            assert!(
                (other.sum - 2.0).abs() < TOLERANCE,
                "case {case}: expected {stream}'s per-shard {SHARD_OUTGOING_RECORDS} to count \
                 the records its read served, got {other:?}"
            );
        }
    }
}

/// RULE, for the push reader: a `SubscribeToShard` delivery publishes its own lag into ITS
/// per-API family, and into the shard-dimensioned lag the stream enhanced — a record leaves the
/// shard the same way whichever reader asked for it — while the pull reader's family counts
/// only pull reads.
#[tokio::test]
async fn a_push_delivery_publishes_its_own_lag_series() {
    let monitored = Monitored::new();
    let shard = shard_id(0);
    let case = "a subscription from the horizon";
    let stream = "lag-pushed";
    active_stream(&monitored, case, stream).await;
    monitored
        .kinesis(
            case,
            "EnableEnhancedMonitoring",
            &json!({ "StreamName": stream, "ShardLevelMetrics": [SHARD_LAG] }),
        )
        .await;
    put_one(&monitored, case, stream).await;
    put_one(&monitored, case, stream).await;

    let consumer = monitored
        .kinesis(
            case,
            "RegisterStreamConsumer",
            &json!({ "StreamARN": stream_arn(REGION, stream), "ConsumerName": "lag-reader" }),
        )
        .await["Consumer"]["ConsumerARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a registration answers a ConsumerARN"))
        .to_string();
    poll_until(case, "the consumer reports ACTIVE", || async {
        let (status, response) = monitored
            .call(
                "kinesis",
                "Kinesis_20131202.DescribeStreamConsumer",
                &json!({ "ConsumerARN": consumer }),
            )
            .await;
        status == 200 && response["ConsumerDescription"]["ConsumerStatus"].as_str() == Some("ACTIVE")
    })
    .await;

    let (status, _) = monitored
        .call(
            "kinesis",
            "Kinesis_20131202.SubscribeToShard",
            &json!({
                "ConsumerARN": consumer,
                "ShardId": shard,
                "StartingPosition": { "Type": "TRIM_HORIZON" },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected the subscription to be established, got {status}"
    );

    // The delivery carried everything the shard held, so its consumer is caught up and the lag
    // it publishes is zero — published, not omitted, under both identities.
    let at_stream = series(&monitored, case, PUSH_LAG, stream_level(stream)).await;
    carries_the_lag(case, PUSH_LAG, &at_stream, 0);
    let at_shard = series(&monitored, case, SHARD_LAG, shard_level(stream, &shard)).await;
    carries_the_lag(case, SHARD_LAG, &at_shard, 0);

    // The pull reader's family counts pull reads, and this stream has had none.
    let pull = series(&monitored, case, PULL_LAG, stream_level(stream)).await;
    assert_eq!(
        pull.datapoints, 0,
        "case {case}: expected no {PULL_LAG} where nothing called GetRecords, got {pull:?}"
    );
}

/// The records a producer put into the stream, which is what the volume of accepted writes is
/// counted into; a member of the enhanced-monitoring catalog ([`EVERY_METRIC`]), so it has a
/// shard-dimensioned form beside the stream-level one.
const INCOMING_RECORDS: &str = "IncomingRecords";

/// The record data those accepted writes carried.
const INCOMING_BYTES: &str = "IncomingBytes";

/// The series a record a shard's write quota turned down is counted into, for the shard that
/// refused it: "Each shard can support writes up to 1,000 records per second, up to a maximum
/// data write total of 1 MB per second" (`PutRecords`), and a record past that rate is not
/// stored. Another member of that same catalog.
const WRITE_THROTTLED: &str = "WriteProvisionedThroughputExceeded";

/// The per-API family counting the entries one `PutRecords` call carried, stored or not.
const BATCH_TOTAL: &str = "PutRecords.TotalRecords";

/// The entries of one such call that were stored.
const BATCH_STORED: &str = "PutRecords.SuccessfulRecords";

/// The entries of one such call that were not.
const BATCH_FAILED: &str = "PutRecords.FailedRecords";

/// The entries of one such call a shard's write quota turned down.
const BATCH_THROTTLED: &str = "PutRecords.ThrottledRecords";

/// Whether one `PutRecord` call stored its record: an observation of 1 for a stored record and
/// of 0 for one the shard turned down, so the SUM over a window is the writes that were taken
/// and the sample count is the calls that asked.
const SINGLE_SUCCESS: &str = "PutRecord.Success";

/// The record data one `PutRecord` call stored.
const SINGLE_BYTES: &str = "PutRecord.Bytes";

/// The unit a tally of records or of calls is published in.
const COUNT: &str = "Count";

/// One mebibyte, the unit the published write-byte rate of a shard is expressed in.
const MI_B: usize = 1024 * 1024;

/// What one series must hold: the `Sum` of its datapoints, carried by `observations`
/// observations — and NO datapoint at all where the calls contributed none of that kind, which
/// is what `observations` of zero asks for.
fn holds(case: &str, metric: &str, reading: &Reading, sum: f64, observations: usize) {
    if observations == 0 {
        assert_eq!(
            reading.datapoints, 0,
            "case {case}: expected no {metric} datapoint where no call contributed one, got \
             {reading:?}"
        );
        return;
    }
    assert!(
        (reading.sum - sum).abs() < TOLERANCE,
        "case {case}: expected {metric} to sum to {sum}, got {reading:?}"
    );
    assert!(
        (reading.sample_count - counted(observations)).abs() < TOLERANCE,
        "case {case}: expected {observations} observation(s) of {metric}, got {reading:?}"
    );
}

/// Every `AWS/Kinesis` metric CloudWatch lists for `stream`, as a client DISCOVERS the series
/// a stream publishes: the names of the series dimensioned on that stream's name, sorted and
/// without repeats.
async fn listed_metrics(monitored: &Monitored, case: &str, stream: &str) -> Vec<String> {
    let answered = monitored
        .cloudwatch(
            case,
            "ListMetrics",
            &json!({
                "Namespace": KINESIS_NAMESPACE,
                "Dimensions": [{ "Name": STREAM_NAME_DIMENSION, "Value": stream }],
            }),
        )
        .await;
    let mut names: Vec<String> = answered["Metrics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|metric| metric["MetricName"].as_str().map(str::to_string))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// One row of the write-accounting table: the batch one `PutRecords` call carries, and whether
/// the shard's published write rate is expected to turn part of it down.
struct BatchRow {
    case: &'static str,
    /// How many entries the call carries.
    entries: usize,
    /// The bytes of data each of those entries carries.
    entry_bytes: usize,
    /// Whether the shard admits only PART of the batch at one instant.
    partial: bool,
}

/// RULE: a data-plane call contributes to the stream's `AWS/Kinesis` series what it ACTUALLY
/// did — the incoming-volume series count the records and the record data the shards STORED, a
/// record a shard's published write rate turned down is counted into the throughput-exceeded
/// series instead of into the volume, and one `PutRecords` call's outcome split
/// (`SuccessfulRecords`, `FailedRecords`, `ThrottledRecords`) reports which way each of the
/// `TotalRecords` entries it carried ended up.
///
/// The rows differ along what the rule generalises over: how many entries one call carries, the
/// bytes each of them holds, which of the two published rates the batch passes, and so whether
/// the shard takes the batch whole or only part of it.
#[tokio::test]
async fn a_batch_counts_what_it_stored_apart_from_what_the_quota_turned_down() {
    assert!(
        EVERY_METRIC.contains(&WRITE_THROTTLED) && EVERY_METRIC.contains(&INCOMING_RECORDS),
        "the volume and the throughput-exceeded counters are members of the \
         enhanced-monitoring catalog, which is what a stream enables their per-shard form \
         through"
    );
    let shard = shard_id(0);
    let rows = [
        // Inside both published rates: the shard takes the whole batch, and nothing is
        // counted as refused.
        BatchRow {
            case: "a batch of tiny entries inside both published rates",
            entries: 20,
            entry_bytes: 8,
            partial: false,
        },
        // Past the write-BYTE rate with many small entries: 300 entries of 16 KiB is more
        // than twice what one shard holds at an instant, so part of the batch is turned down.
        BatchRow {
            case: "a batch of 16 KiB entries past the write-byte rate",
            entries: 300,
            entry_bytes: 16 * 1024,
            partial: true,
        },
        // The same rate reached with FEWER and LARGER entries, so the split does not depend on
        // how many entries the call carries.
        BatchRow {
            case: "a batch of mebibyte entries past the write-byte rate",
            entries: 4,
            entry_bytes: MI_B,
            partial: true,
        },
    ];

    for row in rows {
        let case = row.case;
        // One engine per row, so a row's series hold only that row's own observations, and one
        // PutRecords call per row, so the whole batch is metered at a single instant of the
        // shard's refill rather than at a point that depends on how fast the host runs.
        let monitored = Monitored::new();
        let stream = "write-accounting";
        active_stream(&monitored, case, stream).await;
        // Armed for the volume and for the refusals, so every figure below is checked under
        // BOTH identities the metric has.
        monitored
            .kinesis(
                case,
                "EnableEnhancedMonitoring",
                &json!({
                    "StreamName": stream,
                    "ShardLevelMetrics": [INCOMING_RECORDS, INCOMING_BYTES, WRITE_THROTTLED],
                }),
            )
            .await;

        // One partition key for every entry, so the whole batch is placed in the one shard
        // whose quota the row is about and each entry occupies the same bytes.
        let entries: Vec<Value> = (0..row.entries)
            .map(|_| {
                json!({
                    "PartitionKey": PARTITION_KEY,
                    "Data": aws_smithy_types::base64::encode(vec![b'x'; row.entry_bytes]),
                })
            })
            .collect();
        let answer = monitored
            .kinesis(
                case,
                "PutRecords",
                &json!({ "StreamName": stream, "Records": entries }),
            )
            .await;
        let results = answer["Records"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            results.len(),
            row.entries,
            "case {case}: PutRecords answers one result per entry, got {answer}"
        );
        let stored = results
            .iter()
            .filter(|result| result["ErrorCode"].is_null())
            .count();
        let throttled = results
            .iter()
            .filter(|result| {
                result["ErrorCode"].as_str() == Some(THROUGHPUT_EXCEEDED_ERROR_CODE)
            })
            .count();
        assert_eq!(
            stored + throttled,
            row.entries,
            "case {case}: expected every entry either stored or turned down by the shard's \
             write rate, got {stored} stored and {throttled} refused of {} entries",
            row.entries
        );
        if row.partial {
            assert!(
                stored > 0 && throttled > 0,
                "case {case}: expected the shard to take part of the batch and to turn the \
                 rest down, got {stored} stored and {throttled} refused"
            );
        } else {
            assert_eq!(
                stored, row.entries,
                "case {case}: expected a batch inside both published rates to be stored whole, \
                 got {stored} stored"
            );
        }
        // What a record OCCUPIES: "The record size limit applies to the total size of the
        // partition key and data blob" (`PutRecords`), which is the measure the byte series
        // carry.
        let record_bytes = counted(row.entry_bytes + PARTITION_KEY.len());

        // The volume counts what the shard STORED — not what the call carried — under both
        // identities, and the refusals are counted for the shard that turned them down.
        for dimensions in [stream_level(stream), shard_level(stream, &shard)] {
            let records = series(&monitored, case, INCOMING_RECORDS, dimensions.clone()).await;
            holds(
                case,
                INCOMING_RECORDS,
                &records,
                counted(stored),
                usize::from(stored > 0),
            );
            let bytes = series(&monitored, case, INCOMING_BYTES, dimensions.clone()).await;
            holds(
                case,
                INCOMING_BYTES,
                &bytes,
                counted(stored) * record_bytes,
                usize::from(stored > 0),
            );
            let refused = series(&monitored, case, WRITE_THROTTLED, dimensions).await;
            holds(
                case,
                WRITE_THROTTLED,
                &refused,
                counted(throttled),
                usize::from(throttled > 0),
            );
            assert!(
                refused.units.iter().all(|unit| unit == COUNT),
                "case {case}: expected a tally of refused records in {COUNT}, got {refused:?}"
            );
        }

        // The call's own outcome split: what it carried, what it stored, and what the shard's
        // rate turned down — one observation each, published whether or not the figure is zero,
        // so a client reading the split after the fact can tell an accepted write from a
        // throttled one.
        for (metric, expected) in [
            (BATCH_TOTAL, row.entries),
            (BATCH_STORED, stored),
            (BATCH_FAILED, throttled),
            (BATCH_THROTTLED, throttled),
        ] {
            let reading = series(&monitored, case, metric, stream_level(stream)).await;
            holds(case, metric, &reading, counted(expected), 1);
            // A per-API family is about the CALL, so it exists under the stream's name alone.
            let dimensioned = series(&monitored, case, metric, shard_level(stream, &shard)).await;
            assert_eq!(
                dimensioned.datapoints, 0,
                "case {case}: expected no ShardId-dimensioned {metric}, got {dimensioned:?}"
            );
        }

        // A client finds these series through ListMetrics, which lists one where the call
        // published it and nowhere else — so the throughput-exceeded series is listed for a
        // stream whose shard turned a record down and absent for one that took its batch
        // whole.
        let listed = listed_metrics(&monitored, case, stream).await;
        for metric in [
            INCOMING_RECORDS,
            INCOMING_BYTES,
            BATCH_TOTAL,
            BATCH_STORED,
            BATCH_FAILED,
            BATCH_THROTTLED,
        ] {
            assert!(
                listed.iter().any(|name| name == metric),
                "case {case}: expected ListMetrics to list {metric} for {stream}, got {listed:?}"
            );
        }
        assert_eq!(
            listed.iter().any(|name| name == WRITE_THROTTLED),
            throttled > 0,
            "case {case}: expected ListMetrics to list {WRITE_THROTTLED} exactly where the \
             shard turned a record down ({throttled} refused), got {listed:?}"
        );
    }
}

/// RULE for the single writer: a `PutRecord` the shard's published write rate turned down adds
/// nothing to the volume series and is counted as a refusal — the throughput-exceeded series
/// for the shard, and a zero in the call's own success family — while the write that WAS stored
/// is counted in full.
#[tokio::test]
async fn a_single_write_the_quota_turned_down_is_counted_as_a_refusal_not_as_traffic() {
    let monitored = Monitored::new();
    let case = "two records at the stream's own maximum";
    let stream = "write-accounting-single";
    // A stream configured for the largest record the model admits (`MaxRecordSizeInKiB` is
    // modelled `range 1024..10240`), so ONE record of that size spends ten seconds of the
    // shard's published write rate: the second such record is turned down however fast the host
    // runs, and nothing here waits for a duration.
    let max_record_kib = 10 * 1024_i64;
    let data_bytes = 10 * MI_B;
    active_stream_sized(&monitored, case, stream, Some(max_record_kib)).await;
    monitored
        .kinesis(
            case,
            "EnableEnhancedMonitoring",
            &json!({
                "StreamName": stream,
                "ShardLevelMetrics": [INCOMING_RECORDS, INCOMING_BYTES, WRITE_THROTTLED],
            }),
        )
        .await;
    let shard = shard_id(0);
    let record = json!({
        "StreamName": stream,
        "PartitionKey": PARTITION_KEY,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; data_bytes]),
    });

    let stored_at = monitored.kinesis(case, "PutRecord", &record).await;
    assert!(
        stored_at["SequenceNumber"].is_string(),
        "case {case}: expected the first record stored, got {stored_at}"
    );
    let (status, response) = monitored
        .call("kinesis", "Kinesis_20131202.PutRecord", &record)
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, THROUGHPUT_EXCEEDED_ERROR_CODE),
        "case {case}: expected the second record refused with the modelled throughput error, \
         got {status} with {response}"
    );

    let record_bytes = counted(data_bytes + PARTITION_KEY.len());
    for dimensions in [stream_level(stream), shard_level(stream, &shard)] {
        // One record was stored, so the volume counts exactly that one — the refused write
        // contributed nothing to it.
        let records = series(&monitored, case, INCOMING_RECORDS, dimensions.clone()).await;
        holds(case, INCOMING_RECORDS, &records, 1.0, 1);
        let bytes = series(&monitored, case, INCOMING_BYTES, dimensions.clone()).await;
        holds(case, INCOMING_BYTES, &bytes, record_bytes, 1);
        // ... and the write the shard turned down is counted for that shard.
        let refused = series(&monitored, case, WRITE_THROTTLED, dimensions).await;
        holds(case, WRITE_THROTTLED, &refused, 1.0, 1);
    }

    // The call's own family saw BOTH calls: the stored one as a 1 and the refused one as a 0,
    // so the sum is the writes that were taken while the sample count is the calls that asked.
    let success = series(&monitored, case, SINGLE_SUCCESS, stream_level(stream)).await;
    holds(case, SINGLE_SUCCESS, &success, 1.0, 2);
    // The byte family reports what was STORED, so only the accepted call contributed to it.
    let written = series(&monitored, case, SINGLE_BYTES, stream_level(stream)).await;
    holds(case, SINGLE_BYTES, &written, record_bytes, 1);
}
