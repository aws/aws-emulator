// SPDX-License-Identifier: CC0-1.0

//! What the DESTINATION of a stream with a configured source holds after a direct put to
//! that stream: nothing.
//!
//! The rule under test: `PutRecord` and `PutRecordBatch` are the direct-put ingestion path,
//! and a Firehose stream takes them only when its source IS direct puts — "`DirectPut`:
//! Provider applications access the Firehose stream directly"
//! (`CreateDeliveryStreamInput$DeliveryStreamType`), every other value of that member naming
//! the service that feeds the stream instead. A direct put to a stream of any other type is
//! refused synchronously with the modelled `InvalidArgumentException` (HTTP 400) and the
//! data it carried is never delivered, so the only bytes that can appear at such a stream's
//! destination are the ones its configured source produced.
//!
//! What this suite adds to its siblings: `direct_put_source_gate` pins the REFUSAL over
//! every `DeliveryStreamType` and that nothing is buffered, and
//! `kinesis_source_ingestion::a_source_fed_stream_takes_no_direct_put` pins the destination
//! for `PutRecord` on one `KinesisStreamAsSource` stream. Here the destination is read for
//! every non-direct-put type and for BOTH ingestion operations, with a `DirectPut` stream in
//! the same emulator as the positive control: its records arriving is what proves the
//! delivery clock ran over this store while the refused streams' destinations stayed empty,
//! so the emptiness is evidence rather than an untested wait.
//!
//! Both services are mounted on ONE shared engine by the `destination_delivery` harness, so
//! the Firehose calls go as `POST /` with `X-Amz-Target` and the destination is read with
//! the path-style Amazon S3 requests a client sends.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{at, error_code, error_message, REGION, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// The bucket every stream here delivers into. One bucket, one prefix per stream, so one
/// listing says which stream a delivered object came from.
const BUCKET: &str = "source-gate-delivery-bucket";

/// The `DeliveryStreamType` whose producer is a caller of the ingestion operations.
const DIRECT_PUT: &str = "DirectPut";

/// How long a delivery, or a stream reaching `ACTIVE`, is waited for before a case fails:
/// far longer than the emulator's own spans, so a loaded machine does not fail a case the
/// clock would have served.
const DEADLINE: Duration = Duration::from_secs(30);

/// How often the destination, or a stream's status, is polled while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One stream of the table: the type it is created with, the source configuration that type
/// is fed by, and the marker its direct-put payloads carry.
struct Row {
    /// The stream name, which is also the case label.
    stream: &'static str,
    /// The `DeliveryStreamType` the create declares, or `None` to leave the member out —
    /// which is `DirectPut`, the documented default.
    stream_type: Option<&'static str>,
    /// The `DeliveryStreamType` the stream ends up carrying: what a describe reports and,
    /// for a refused row, what the refusal names.
    stored_type: &'static str,
    /// The destination `Prefix` this stream's deliveries land under.
    prefix: &'static str,
    /// The byte marker this row's payloads carry, so a delivered object's bytes say which
    /// stream's put produced it.
    marker: &'static str,
}

/// The table: one row per value of `DeliveryStreamType`
/// (`smithy_query(query="shape",shape="DeliveryStreamType")` -> ["DirectPut",
/// "KinesisStreamAsSource", "MSKAsSource", "DatabaseAsSource"]), plus the member left out
/// altogether for the documented default. The direct-put rows are the positive control; the
/// rest name a source service and must take nothing.
fn rows() -> Vec<Row> {
    vec![
        Row {
            stream: "gate-direct-put-declared",
            stream_type: Some(DIRECT_PUT),
            stored_type: DIRECT_PUT,
            prefix: "declared/",
            marker: "declared-direct-put",
        },
        Row {
            stream: "gate-direct-put-by-default",
            stream_type: None,
            stored_type: DIRECT_PUT,
            prefix: "default/",
            marker: "default-direct-put",
        },
        Row {
            stream: "gate-kinesis-backed",
            stream_type: Some("KinesisStreamAsSource"),
            stored_type: "KinesisStreamAsSource",
            prefix: "kinesis/",
            marker: "kinesis-backed-direct-put",
        },
        Row {
            stream: "gate-msk-backed",
            stream_type: Some("MSKAsSource"),
            stored_type: "MSKAsSource",
            prefix: "msk/",
            marker: "msk-backed-direct-put",
        },
        Row {
            stream: "gate-database-backed",
            stream_type: Some("DatabaseAsSource"),
            stored_type: "DatabaseAsSource",
            prefix: "database/",
            marker: "database-backed-direct-put",
        },
    ]
}

/// The source configuration member a declared type is fed by, and its value; `None` for a
/// type that is fed by direct puts.
///
/// Every member is the one `CreateDeliveryStream` pairs with that type
/// (`smithy_query(query="operation",operation="CreateDeliveryStream")`), carrying its
/// @required members.
fn source_of(stream_type: Option<&str>, stream: &str) -> Option<(&'static str, Value)> {
    match stream_type {
        Some("KinesisStreamAsSource") => Some((
            "KinesisStreamSourceConfiguration",
            json!({
                "KinesisStreamARN":
                    format!("arn:aws:kinesis:{REGION}:000000000000:stream/{stream}"),
                "RoleARN": ROLE_ARN,
            }),
        )),
        Some("MSKAsSource") => Some((
            "MSKSourceConfiguration",
            json!({
                "MSKClusterARN": format!("arn:aws:kafka:{REGION}:000000000000:cluster/{stream}/1"),
                "TopicName": "events",
                "AuthenticationConfiguration": { "RoleARN": ROLE_ARN, "Connectivity": "PRIVATE" },
            }),
        )),
        Some("DatabaseAsSource") => Some((
            "DatabaseSourceConfiguration",
            json!({
                "Type": "MySQL",
                "Endpoint": "db.example.internal",
                "Port": 3306,
                "SSLMode": "Enabled",
                "Databases": { "Include": ["sales"] },
                "Tables": { "Include": ["orders"] },
                "SnapshotWatermarkTable": "sales.firehose_watermark",
                "DatabaseSourceAuthenticationConfiguration": {
                    "SecretsManagerConfiguration": {
                        "SecretARN":
                            format!("arn:aws:secretsmanager:{REGION}:000000000000:secret:db-cred"),
                        "RoleARN": ROLE_ARN,
                        "Enabled": true,
                    },
                },
                "DatabaseSourceVPCConfiguration": {
                    "VpcEndpointServiceName":
                        "com.amazonaws.vpce.us-west-2.vpce-svc-01234567890abcdef",
                },
            }),
        )),
        _ => None,
    }
}

/// The payload of round `round` for `row`: its marker, so a delivered object's bytes name
/// the stream whose put produced it, and the round it was written in.
fn payload(row: &Row, round: &str) -> Vec<u8> {
    format!("{{\"marker\":\"{}\",\"round\":\"{round}\"}}", row.marker).into_bytes()
}

#[tokio::test]
async fn a_configured_sources_destination_takes_nothing_from_a_direct_put() {
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;
    let rows = rows();

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row.
    for row in &rows {
        let mut request = json!({ "DeliveryStreamName": row.stream });
        if let Some(stream_type) = row.stream_type {
            request["DeliveryStreamType"] = json!(stream_type);
        }
        if let Some((member, configuration)) = source_of(row.stream_type, row.stream) {
            request[member] = configuration;
        }
        request["ExtendedS3DestinationConfiguration"] = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
            "Prefix": row.prefix,
            // The shortest wait `IntervalInSeconds` admits (published range 0..900): what
            // these cases read is WHICH records reach the destination, not how long a
            // buffer is held.
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        });
        emulator.firehose_ok("CreateDeliveryStream", request).await;
    }
    for row in &rows {
        await_active(&emulator, row).await;
    }

    // Round one: one `PutRecord` into every stream. A direct-put stream takes it; a stream
    // with a configured source refuses it, naming the type that stands in the way.
    for row in &rows {
        let (status, answered) = emulator.put_record(row.stream, &payload(row, "one")).await;
        assert_outcome(row, "PutRecord", status, &answered);
    }
    // The control streams' records arriving is what says the delivery clock has run over
    // this store; the refused streams' destinations are read only after it has.
    await_delivered(&emulator, &rows, 1).await;
    assert_only_direct_put_delivered(&emulator, &rows, "after PutRecord").await;

    // Round two: a `PutRecordBatch` into every stream, which the gate must answer the same
    // way — the whole request refused, rather than per-record failures.
    for row in &rows {
        let records = vec![payload(row, "two-first"), payload(row, "two-second")];
        let (status, answered) = emulator.put_batch(row.stream, &records).await;
        assert_outcome(row, "PutRecordBatch", status, &answered);
    }
    // A second delivery window, awaited the same way: by then the refused streams have had
    // two of them and still hold nothing.
    await_delivered(&emulator, &rows, 2).await;
    assert_only_direct_put_delivered(&emulator, &rows, "after PutRecordBatch").await;
}

/// Assert the ingestion call `operation` was answered as `row`'s source type requires:
/// served for a direct-put stream, refused with the modelled `InvalidArgumentException` for
/// a stream fed by a configured source.
fn assert_outcome(row: &Row, operation: &str, status: u16, answered: &Value) {
    let case = row.stream;
    if row.stored_type == DIRECT_PUT {
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} on a direct-put stream is served, got {status} with \
             {answered:?}"
        );
        return;
    }
    assert_eq!(
        status, 400,
        "case {case:?}: {operation} on a {} stream is refused, got {status} with {answered:?}",
        row.stored_type
    );
    let code = error_code(answered);
    assert_eq!(
        code, "InvalidArgumentException",
        "case {case:?}: {operation} got {code:?} in {answered:?}"
    );
    // The sentence the data plane states for this refusal, built from the one fact that
    // decides it: the `DeliveryStreamType` the stream carries.
    let expected = format!(
        "This operation is not permitted on {} delivery stream type.",
        row.stored_type
    );
    let message = error_message(answered);
    assert_eq!(
        message, expected,
        "case {case:?}: {operation} expected {expected:?}, got {message:?}"
    );
    // A refused request is answered with the error alone: no record id, and no per-record
    // envelope reporting part of the batch as taken.
    assert!(
        answered.get("RecordId").is_none() && answered.get("RequestResponses").is_none(),
        "case {case:?}: a refused {operation} reports no stored record, got {answered:?}"
    );
}

/// Wait for `row`'s stream to report `ACTIVE`, polling its describe until it does.
async fn await_active(emulator: &Emulator, row: &Row) {
    let case = row.stream;
    let deadline = Instant::now() + DEADLINE;
    loop {
        let (status, described) = emulator
            .firehose(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": row.stream }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: describe got {described:?}");
        let description = at(&described, "DeliveryStreamDescription/DeliveryStreamStatus");
        if description == &json!("ACTIVE") {
            // The type the gate and the refusal read is the stored one, which is what the
            // describe reports.
            let reported = at(&described, "DeliveryStreamDescription/DeliveryStreamType");
            assert_eq!(
                reported,
                &json!(row.stored_type),
                "case {case:?}: the stored type is what the create declared, got {described:?}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stream must reach ACTIVE, got {described:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until every direct-put row's prefix holds at least `want` delivered objects.
async fn await_delivered(emulator: &Emulator, rows: &[Row], want: usize) {
    for row in rows.iter().filter(|row| row.stored_type == DIRECT_PUT) {
        let case = row.stream;
        let deadline = Instant::now() + DEADLINE;
        loop {
            let keys = emulator.keys_under(BUCKET, row.prefix).await;
            if keys.len() >= want {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected at least {want} delivered object(s) under {:?}, got \
                 {keys:?}",
                row.prefix
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// Assert the bucket holds the direct-put streams' data and nothing a refused put carried:
/// no object under a source-fed stream's prefix, and no refused marker in the bytes of ANY
/// object, wherever it was written.
async fn assert_only_direct_put_delivered(emulator: &Emulator, rows: &[Row], stage: &str) {
    let bodies = emulator.delivered_bodies(BUCKET).await;
    let held: Vec<u8> = bodies.values().flatten().copied().collect();
    let keys: Vec<&String> = bodies.keys().collect();
    for row in rows {
        let under_prefix = emulator.keys_under(BUCKET, row.prefix).await;
        let marker_delivered = contains(&held, row.marker.as_bytes());
        if row.stored_type == DIRECT_PUT {
            assert!(
                !under_prefix.is_empty(),
                "case {:?} ({stage}): a direct-put stream's records are delivered under {:?}, \
                 got {keys:?}",
                row.stream,
                row.prefix
            );
            assert!(
                marker_delivered,
                "case {:?} ({stage}): the delivered objects hold this stream's payloads, got \
                 {keys:?}",
                row.stream
            );
        } else {
            assert!(
                under_prefix.is_empty(),
                "case {:?} ({stage}): a {} stream's destination takes nothing from a direct \
                 put, got {under_prefix:?}",
                row.stream,
                row.stored_type
            );
            assert!(
                !marker_delivered,
                "case {:?} ({stage}): the marker of a refused put is nowhere in {BUCKET}, got \
                 {keys:?}",
                row.stream
            );
        }
    }
}

/// Whether `haystack` holds `needle` anywhere in it.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
