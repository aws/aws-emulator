// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS/Logs` metric data an INGESTION publishes: what the log group
//! took in, and the Embedded Metric Format documents it could not read.
//!
//! The rules under test:
//!
//! * **every `PutLogEvents` that stores events publishes, for the log group it stored them in,
//!   how many they were (`IncomingLogEvents`) and what they weighed in uncompressed bytes
//!   (`IncomingBytes`) — the service's own metrics, in the `AWS/Logs` namespace under the
//!   `LogGroupName` dimension — and a call that stored nothing publishes neither;**
//! * **an ingested log event that declared itself an Embedded Metric Format document (it
//!   carries the format's `_aws` member) whose metadata this service could not read counts one
//!   `EMFParsingErrors` for that log group, while an event that is not such a document counts
//!   none.**
//!
//! Every write goes over the wire — `POST /` with `X-Amz-Target: Logs_20140328.PutLogEvents`
//! through the router that mounts this service — so a datapoint that was not published by the
//! ingestion cannot appear. What they are READ from is the CloudWatch emulator's own metric
//! store on the shared engine, which is where they have to land for `GetMetricStatistics` to
//! answer with them.
//!
//! The rows span what the rules generalise over: how many events a batch stores, how many bytes
//! their messages weigh (including a multi-byte message, where bytes and characters differ), a
//! batch some of whose events the ingestion window refuses, repeated calls, and each way an
//! Embedded Metric Format document can fail to read against documents that read perfectly well.

use serde_json::{json, Value};

use crate::harness::{base_ms, group_with_stream, Logs};
use crate::metric_filter_extraction::published;

/// The namespace this service publishes its own metrics into: "the AWS/Logs namespace".
const LOGS_NAMESPACE: &str = "AWS/Logs";

/// The metrics an ingestion publishes.
const INCOMING_LOG_EVENTS: &str = "IncomingLogEvents";
const INCOMING_BYTES: &str = "IncomingBytes";
const EMF_PARSING_ERRORS: &str = "EMFParsingErrors";

/// The dimension they are published under: the log group that took the events in.
const LOG_GROUP_NAME_DIMENSION: &str = "LogGroupName";

/// The values published for one metric of this service, for the log group `group` — in the
/// order they were published.
fn values_for(logs: &Logs, metric: &str, group: &str) -> Vec<f64> {
    published(logs, LOGS_NAMESPACE, metric)
        .into_iter()
        .filter(|datapoint| {
            datapoint.dimensions.get(LOG_GROUP_NAME_DIMENSION) == Some(&group.to_string())
        })
        .map(|datapoint| datapoint.value)
        .collect()
}

/// The dimension sets one metric of this service was published under, for every log group.
fn dimension_sets(logs: &Logs, metric: &str) -> Vec<Vec<(String, String)>> {
    published(logs, LOGS_NAMESPACE, metric)
        .into_iter()
        .map(|datapoint| datapoint.dimensions.into_iter().collect())
        .collect()
}

/// Ingest `events` as one batch into `group`'s one stream.
async fn ingest(logs: &Logs, group: &str, events: &[(i64, &str)]) -> (u16, Value) {
    logs.call(
        "PutLogEvents",
        &json!({
            "logGroupName": group,
            "logStreamName": "events",
            "logEvents": crate::harness::batch(events),
        }),
    )
    .await
}

/// One row of the ingestion table: the messages one batch carries, and what the ingestion must
/// count for them.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The messages of the batch, all stamped inside the ingestion window.
    messages: &'static [&'static str],
}

impl Row {
    /// The bytes the messages weigh: their own UTF-8 bytes, which is what "uncompressed bytes"
    /// counts and what differs from a count of characters for a multi-byte message.
    fn bytes(&self) -> f64 {
        self.messages
            .iter()
            .map(|message| message.len())
            .sum::<usize>() as f64
    }
}

/// Every batch publishes the count of the events it stored and the bytes they weighed, under
/// the log group that took them in.
#[tokio::test]
async fn an_ingestion_publishes_what_the_log_group_took_in() {
    let rows = [
        Row {
            case: "one event",
            messages: &["the first line"],
        },
        Row {
            case: "three events of different lengths",
            messages: &["a", "a longer line", "the longest line of the three"],
        },
        Row {
            case: "a multi-byte message, where bytes and characters differ",
            messages: &["dróżka — ścieżka"],
        },
        Row {
            case: "a message that is an Embedded Metric Format document",
            messages: &[
                r#"{"_aws":{"CloudWatchMetrics":[{"Namespace":"App","Metrics":[{"Name":"Latency"}],"Dimensions":[[]]}]},"Latency":12}"#,
            ],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let logs = Logs::new();
        let group = format!("ingestion/metrics-{index}");
        group_with_stream(&logs, &group, "events", None).await;
        let at = base_ms();
        let events: Vec<(i64, &str)> = row
            .messages
            .iter()
            .enumerate()
            .map(|(offset, message)| (at + offset as i64, *message))
            .collect();

        let (status, parsed) = ingest(&logs, &group, &events).await;
        assert_eq!(
            status, 200,
            "case {:?}: the batch must be accepted, got {status} {parsed}",
            row.case
        );

        let counted = values_for(&logs, INCOMING_LOG_EVENTS, &group);
        assert_eq!(
            counted,
            vec![row.messages.len() as f64],
            "case {:?}: expected one datapoint counting {} events, got {counted:?}",
            row.case,
            row.messages.len()
        );
        let weighed = values_for(&logs, INCOMING_BYTES, &group);
        assert_eq!(
            weighed,
            vec![row.bytes()],
            "case {:?}: expected one datapoint of {} bytes, got {weighed:?}",
            row.case,
            row.bytes()
        );
        let sets = dimension_sets(&logs, INCOMING_LOG_EVENTS);
        assert_eq!(
            sets,
            vec![vec![(
                LOG_GROUP_NAME_DIMENSION.to_string(),
                group.clone()
            )]],
            "case {:?}: the count is published under the log group alone, got {sets:?}",
            row.case
        );
    }
}

/// Each call publishes its own datapoints, and a call that stored nothing publishes none: an
/// event outside the ingestion window is not an incoming log event.
#[tokio::test]
async fn only_the_events_an_ingestion_stored_are_counted() {
    let logs = Logs::new();
    let group = "ingestion/window";
    group_with_stream(&logs, group, "events", None).await;
    let at = base_ms();

    // Two batches, so the metric accumulates per call rather than being overwritten.
    ingest(&logs, group, &[(at, "first"), (at + 1, "second")]).await;
    ingest(&logs, group, &[(at + 2, "third")]).await;
    let counted = values_for(&logs, INCOMING_LOG_EVENTS, group);
    assert_eq!(
        counted,
        vec![1.0, 2.0],
        "case \"two batches\": expected a datapoint per call, got {counted:?}"
    );

    // An event more than 14 days old is outside the ingestion window, so nothing of that
    // batch is stored — and nothing of it is counted.
    let too_old = at - 20 * 24 * 60 * 60 * 1_000;
    let (status, parsed) = ingest(&logs, group, &[(too_old, "far too old")]).await;
    assert_eq!(
        status, 200,
        "case \"an event outside the window\": the call is accepted, got {status} {parsed}"
    );
    assert!(
        parsed.get("rejectedLogEventsInfo").is_some(),
        "case \"an event outside the window\": the call reports the rejected event, got {parsed}"
    );
    let after = values_for(&logs, INCOMING_LOG_EVENTS, group);
    assert_eq!(
        after, counted,
        "case \"an event outside the window\": nothing stored, so nothing counted, got {after:?}"
    );
}

/// One row of the Embedded Metric Format table: a message, and whether the ingestion counts a
/// parsing error for it.
struct EmfRow {
    /// What the row varies, named in every assertion.
    case: &'static str,
    /// The message ingested.
    message: &'static str,
    /// Whether this service could not read the document the message declared.
    unreadable: bool,
}

/// An event that declared itself an Embedded Metric Format document and could not be read as
/// one counts a parsing error for its log group; one that reads, and one that is no such
/// document at all, count none.
#[tokio::test]
async fn an_unreadable_embedded_metric_format_document_counts_a_parsing_error() {
    let rows = [
        EmfRow {
            case: "a document that reads and declares a metric",
            message: r#"{"_aws":{"CloudWatchMetrics":[{"Namespace":"App","Metrics":[{"Name":"Latency"}],"Dimensions":[[]]}]},"Latency":12}"#,
            unreadable: false,
        },
        EmfRow {
            case: "a plain log line",
            message: "ERROR something happened",
            unreadable: false,
        },
        EmfRow {
            case: "a JSON document carrying no _aws member",
            message: r#"{"level":"error","Latency":12}"#,
            unreadable: false,
        },
        EmfRow {
            case: "an _aws member that is not an object",
            message: r#"{"_aws":"metrics","Latency":12}"#,
            unreadable: true,
        },
        EmfRow {
            case: "metadata declaring no CloudWatchMetrics",
            message: r#"{"_aws":{"Timestamp":1700000000000},"Latency":12}"#,
            unreadable: true,
        },
        EmfRow {
            case: "a CloudWatchMetrics that is not a list",
            message: r#"{"_aws":{"CloudWatchMetrics":"App"},"Latency":12}"#,
            unreadable: true,
        },
        EmfRow {
            case: "an empty CloudWatchMetrics list",
            message: r#"{"_aws":{"CloudWatchMetrics":[]},"Latency":12}"#,
            unreadable: true,
        },
        EmfRow {
            case: "a directive naming no namespace",
            message: r#"{"_aws":{"CloudWatchMetrics":[{"Metrics":[{"Name":"Latency"}]}]},"Latency":12}"#,
            unreadable: true,
        },
        EmfRow {
            case: "a directive declaring no metrics list",
            message: r#"{"_aws":{"CloudWatchMetrics":[{"Namespace":"App"}]},"Latency":12}"#,
            unreadable: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let logs = Logs::new();
        let group = format!("ingestion/emf-{index}");
        group_with_stream(&logs, &group, "events", None).await;
        let at = base_ms();

        let (status, parsed) = ingest(&logs, &group, &[(at, row.message)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the event must be accepted, got {status} {parsed}",
            row.case
        );

        let errors = values_for(&logs, EMF_PARSING_ERRORS, &group);
        let expected: Vec<f64> = if row.unreadable { vec![1.0] } else { Vec::new() };
        assert_eq!(
            errors, expected,
            "case {:?}: expected {expected:?} parsing errors, got {errors:?}",
            row.case
        );
        // Whatever the document did, the ingestion itself is still counted: one event in.
        let counted = values_for(&logs, INCOMING_LOG_EVENTS, &group);
        assert_eq!(
            counted,
            vec![1.0],
            "case {:?}: the event is counted as incoming whatever it carried, got {counted:?}",
            row.case
        );
    }
}
