// SPDX-License-Identifier: CC0-1.0

//! The HEALTH CHECK half of configuring a subscription filter, end to end over the wire with
//! the Kinesis emulator mounted beside this service on one engine — so the record is observed
//! where a consumer observes it: as a record the destination's shard serves.
//!
//! RULE: a `PutSubscriptionFilter` that is accepted WRITES to the destination it configures
//! the filter against — one gzip-framed health-check envelope, before any log event of that
//! filter exists, marked as a message of the control plane and owned by CloudWatch Logs
//! itself, naming no log group, no log stream and no subscription filter, and carrying one
//! log event whose message opens with the service's fixed health-check sentence. The check
//! belongs to the put, so it is written once per accepted put, whatever the filter's pattern,
//! distribution or destination kind, and it lands on the destination's shard BEFORE the
//! records any later ingestion delivers.
//!
//! The recorded flow this was derived from puts one filter with an empty pattern on a Kinesis
//! stream named directly, then writes three lines; that is one row of the table below. The
//! rows differ along what the rule generalises over: whether the destination is named
//! directly or through a logical destination, the FORM of the filter pattern (match
//! everything, an unstructured term, a term nothing in the group will carry), the
//! `distribution` the filter is configured with, and whether the put CREATES the filter or
//! updates one already held.
//!
//! The second test is the rest of the same rule: the check precedes the data on the shard's
//! own sequence numbers, and the ids the delivered envelope carries are the `eventId`s
//! `FilterLogEvents` reports for the same stored events.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::sequence_number_le;

use crate::harness::base_ms;
use crate::subscription_fan_out::{
    gunzip, past_activation, role_arn, stream_arn, Emulator, BY_LOG_STREAM, CONTROL_MESSAGE,
    DATA_MESSAGE,
};

/// The `owner` a health-check envelope carries: the service that wrote it, not the account
/// whose filter it validates.
const CONTROL_MESSAGE_OWNER: &str = "CloudwatchLogs";

/// The fixed text a health check's one log event opens with.
const CONTROL_MESSAGE_PREFIX: &str = "CWL CONTROL MESSAGE: Checking health of destination";

/// The log stream the rows that write events write to.
const LOG_STREAM: &str = "events";

/// One row of the health-check table.
struct Row {
    /// What the row varies, named in every assertion it makes.
    case: &'static str,
    /// The log group the row configures its filter on, one per row so its destination holds
    /// only its own records.
    group: &'static str,
    /// The Kinesis stream the row's destination resolves to, one per row for the same reason.
    stream: &'static str,
    /// The filter's name.
    filter: &'static str,
    /// The filter's `filterPattern`, which must change nothing about the check.
    pattern: &'static str,
    /// The `distribution` the filter is put with, or `None` to leave it at the default.
    distribution: Option<&'static str>,
    /// Whether the filter names its destination through a `PutDestination` logical
    /// destination rather than the stream itself.
    through_logical_destination: bool,
    /// How many times the row puts the filter: one check per accepted put, so a re-put of
    /// the same filter re-validates the destination and writes another.
    puts: usize,
}

/// The health-check table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: one filter, an empty pattern, the stream named directly.
            case: "a match-everything filter on a stream named directly",
            group: "/aws/emulator/health-direct",
            stream: "health-direct",
            filter: "direct-filter",
            pattern: "",
            distribution: None,
            through_logical_destination: false,
            puts: 1,
        },
        Row {
            case: "an unstructured term through a logical destination",
            group: "/aws/emulator/health-logical",
            stream: "health-logical",
            filter: "logical-filter",
            pattern: "SIM2REALMATCH",
            distribution: None,
            through_logical_destination: true,
            puts: 1,
        },
        Row {
            case: "a term nothing in the group will carry, grouped by log stream",
            group: "/aws/emulator/health-quiet",
            stream: "health-quiet",
            filter: "quiet-filter",
            pattern: "NOTHING_MATCHES_THIS",
            distribution: Some(BY_LOG_STREAM),
            through_logical_destination: false,
            puts: 1,
        },
        Row {
            case: "a filter put twice, so the destination is validated twice",
            group: "/aws/emulator/health-reput",
            stream: "health-reput",
            filter: "reput-filter",
            pattern: "",
            distribution: None,
            through_logical_destination: false,
            puts: 2,
        },
    ]
}

/// The log group and the log stream one row writes into.
async fn given_log_stream(emulator: &Emulator, case: &str, group: &str) {
    emulator
        .logs(case, "CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    emulator
        .logs(
            case,
            "CreateLogStream",
            &json!({"logGroupName": group, "logStreamName": LOG_STREAM}),
        )
        .await;
}

/// Assert one health-check envelope against the rule, whatever produced it.
fn assert_health_check(case: &str, envelope: &Value) {
    assert_eq!(
        envelope["messageType"].as_str(),
        Some(CONTROL_MESSAGE),
        "case {case}: a health check is a {CONTROL_MESSAGE}, got {envelope}"
    );
    assert_eq!(
        envelope["owner"].as_str(),
        Some(CONTROL_MESSAGE_OWNER),
        "case {case}: a health check is owned by {CONTROL_MESSAGE_OWNER}, got {envelope}"
    );
    assert_eq!(
        envelope["logGroup"].as_str(),
        Some(""),
        "case {case}: a health check names no log group, got {envelope}"
    );
    assert_eq!(
        envelope["logStream"].as_str(),
        Some(""),
        "case {case}: a health check names no log stream, got {envelope}"
    );
    assert_eq!(
        envelope["subscriptionFilters"].as_array(),
        Some(&Vec::new()),
        "case {case}: a health check names no subscription filter, got {envelope}"
    );
    let events = envelope["logEvents"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a health check carries logEvents, got {envelope}"));
    assert_eq!(
        events.len(),
        1,
        "case {case}: a health check carries exactly one log event, got {envelope}"
    );
    let message = events[0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the check's event carries a message, got {envelope}"));
    assert!(
        message.starts_with(CONTROL_MESSAGE_PREFIX),
        "case {case}: expected a message opening {CONTROL_MESSAGE_PREFIX:?}, got {message:?}"
    );
    assert!(
        events[0]["timestamp"].as_i64().is_some_and(|at| at > 0),
        "case {case}: the check's event is stamped with the instant it was made, got {envelope}"
    );
}

/// Every accepted put writes one health check to the destination it configures, and nothing
/// else reaches that destination until something is ingested.
#[tokio::test]
async fn an_accepted_put_writes_one_health_check_to_its_destination() {
    let emulator = Emulator::new();
    let rows = rows();
    for row in &rows {
        emulator.create_stream(row.case, row.stream).await;
    }
    // Every stream is WAITED FOR until it reports itself usable, rather than for a span of
    // the clock: the status it leaves `CREATING` for is one the clock owes, and the wait ends
    // as soon as the stream says so.
    for row in &rows {
        emulator.await_active_stream(row.case, row.stream).await;
    }

    for row in &rows {
        let case = row.case;
        given_log_stream(&emulator, case, row.group).await;
        let destination = if row.through_logical_destination {
            emulator
                .logical_destination(case, &format!("{}-destination", row.filter), row.stream)
                .await
        } else {
            stream_arn(row.stream)
        };
        let mut put = json!({
            "logGroupName": row.group,
            "filterName": row.filter,
            "filterPattern": row.pattern,
            "destinationArn": destination,
        });
        if !row.through_logical_destination {
            // A logical destination carries the role it delivers through; a stream named
            // directly needs the filter to name one.
            put["roleArn"] = json!(role_arn());
        }
        if let Some(distribution) = row.distribution {
            put["distribution"] = json!(distribution);
        }
        for _ in 0..row.puts {
            emulator.logs(case, "PutSubscriptionFilter", &put).await;
        }
    }

    for row in &rows {
        let case = row.case;
        let checks = emulator.health_checks(case, row.stream).await;
        // One check per accepted put — and, for a row that goes through a logical
        // destination, the test message `PutDestination` wrote while it validated that
        // destination, which is the same control record.
        let expected = row.puts + usize::from(row.through_logical_destination);
        assert_eq!(
            checks.len(),
            expected,
            "case {case}: {} accepted put(s) and {} destination(s) write one control record \
             each, got {checks:?}",
            row.puts,
            usize::from(row.through_logical_destination)
        );
        for (key, envelope) in &checks {
            assert_health_check(case, envelope);
            assert!(
                !key.is_empty(),
                "case {case}: a health-check record carries a partition key, got {key:?}"
            );
        }
        let delivered = emulator.delivered(case, row.stream).await;
        assert!(
            delivered.is_empty(),
            "case {case}: no log event was ingested, so the destination holds no data record, \
             got {delivered:?}"
        );
    }
}

/// Every record the one-shard stream holds, in shard order: its sequence number and the
/// envelope its gzip frame decodes to. The shard drain itself is the harness's
/// ([`Emulator::sequenced_shard_records`]); this only decodes what it answered.
async fn sequenced(emulator: &Emulator, case: &str, stream: &str) -> Vec<(String, Value)> {
    emulator
        .sequenced_shard_records(case, stream)
        .await
        .into_iter()
        .map(|(sequence_number, _key, framed)| {
            let body = gunzip(case, &framed);
            let envelope: Value = serde_json::from_slice(&body)
                .unwrap_or_else(|error| panic!("case {case}: the frame holds JSON, got {error}"));
            (sequence_number, envelope)
        })
        .collect()
}

/// The lines one batch writes, each on its own millisecond.
const LINES: [&str; 3] = [
    "SIM2REALMATCH alpha",
    "SIM2REALMATCH beta",
    "SIM2REALMATCH gamma",
];

/// The check lands on the shard before the data, and the ids the delivered envelope carries
/// are the `eventId`s `FilterLogEvents` reports for the same stored events.
#[tokio::test]
async fn the_health_check_precedes_the_data_and_the_ids_join_to_filter_log_events() {
    let case = "a filter validated, then three lines written";
    let emulator = Emulator::new();
    let stream = "health-then-data";
    let group = "/aws/emulator/health-then-data";
    let filter = "joined-filter";
    emulator.create_stream(case, stream).await;
    tokio::time::sleep(past_activation()).await;
    given_log_stream(&emulator, case, group).await;

    emulator
        .logs(
            case,
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": filter,
                "filterPattern": "",
                "destinationArn": stream_arn(stream),
                "roleArn": role_arn(),
            }),
        )
        .await;

    let at = base_ms();
    let events: Vec<Value> = LINES
        .iter()
        .enumerate()
        .map(|(nth, message)| json!({"timestamp": at + nth as i64, "message": message}))
        .collect();
    emulator
        .logs(
            case,
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": LOG_STREAM,
                "logEvents": events,
            }),
        )
        .await;

    let records = sequenced(&emulator, case, stream).await;
    let control: Vec<&(String, Value)> = records
        .iter()
        .filter(|(_, envelope)| envelope["messageType"] == json!(CONTROL_MESSAGE))
        .collect();
    let data: Vec<&(String, Value)> = records
        .iter()
        .filter(|(_, envelope)| envelope["messageType"] == json!(DATA_MESSAGE))
        .collect();
    assert_eq!(
        control.len(),
        1,
        "case {case}: the put wrote one health check, got {records:?}"
    );
    assert!(
        !data.is_empty(),
        "case {case}: the batch owed at least one data record, got {records:?}"
    );
    let (control_at, control_envelope) = control[0];
    assert_health_check(case, control_envelope);
    let (first_data_at, _) = data[0];
    assert!(
        sequence_number_le(control_at, first_data_at) && control_at != first_data_at,
        "case {case}: the check's sequence number is lower than the first data record's, got \
         {control_at} then {first_data_at}"
    );

    // Every id the envelopes carried, in delivery order.
    let delivered_ids: Vec<String> = data
        .iter()
        .flat_map(|(_, envelope)| {
            envelope["logEvents"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|event| event["id"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>()
        })
        .collect();
    let searched = emulator
        .logs(
            case,
            "FilterLogEvents",
            &json!({"logGroupName": group}),
        )
        .await;
    let reported: Vec<(String, String)> = searched["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| {
            (
                event["message"].as_str().unwrap_or_default().to_string(),
                event["eventId"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        reported.iter().map(|(message, _)| message.clone()).collect::<Vec<String>>(),
        LINES.map(str::to_string).to_vec(),
        "case {case}: FilterLogEvents reports the lines that were written, got {searched}"
    );
    assert_eq!(
        delivered_ids,
        reported.iter().map(|(_, id)| id.clone()).collect::<Vec<String>>(),
        "case {case}: a delivered event's id is the eventId FilterLogEvents reports for the \
         same stored event, got {delivered_ids:?} against {reported:?}"
    );
}
