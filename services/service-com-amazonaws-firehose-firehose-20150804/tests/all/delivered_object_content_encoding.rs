// SPDX-License-Identifier: CC0-1.0

//! What a delivered Amazon S3 object says its bytes are: `Content-Type` and
//! `Content-Encoding`.
//!
//! The rule under test: Firehose encodes a delivered object's body as the destination's
//! `CompressionFormat` asks ("the compression format. If no value is specified, the default
//! is UNCOMPRESSED" — `ExtendedS3DestinationConfiguration$CompressionFormat`) and states that
//! encoding on the object it writes, as the content coding the format names — the format's
//! own value in lower case — while an `UNCOMPRESSED` object carries no coding at all. The
//! media type is the same for every format: Firehose does not interpret the records' bytes,
//! so a delivered object is an opaque byte stream.
//!
//! So a consumer that reads the object's metadata to choose a decoder is told which one the
//! bytes are for, and can tell a `zip` object from a `gzip` one.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, ROLE_ARN};
use crate::destination_delivery::Emulator;

/// The bucket every case here delivers into.
const BUCKET: &str = "content-encoding-bucket";

/// How long a delivery is waited for before a case fails.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The media type every delivered object carries, whatever it was encoded with.
const OCTET_STREAM: &str = "application/octet-stream";

/// The record every case puts. Its bytes decide nothing here beyond being long enough that
/// an encoder has something to do with them.
const RECORD: &[u8] = b"{\"event\":\"delivered\",\"payload\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}";

/// One compression case: what the destination declares, and the coding the delivered object
/// must then report.
struct Row {
    case: &'static str,
    /// The `CompressionFormat` the destination configures, `None` to leave the member out.
    declared: Option<&'static str>,
    /// The `Content-Encoding` the delivered object must carry, `None` for none at all.
    coding: Option<&'static str>,
}

/// The compression cases.
///
/// One row per value the `CompressionFormat` enum admits (`smithy_query(query="shape",
/// shape="CompressionFormat")`: `UNCOMPRESSED`, `GZIP`, `ZIP`, `Snappy`,
/// `HADOOP_SNAPPY`), plus the member left out altogether for the documented default. Each
/// compressed row's coding is the declared format in lower case; `UNCOMPRESSED` and the
/// absent member carry none.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no CompressionFormat at all, the documented default",
            declared: None,
            coding: None,
        },
        Row {
            case: "UNCOMPRESSED named outright",
            declared: Some("UNCOMPRESSED"),
            coding: None,
        },
        Row {
            case: "GZIP",
            declared: Some("GZIP"),
            coding: Some("gzip"),
        },
        Row {
            case: "ZIP",
            declared: Some("ZIP"),
            coding: Some("zip"),
        },
        Row {
            case: "Snappy",
            declared: Some("Snappy"),
            coding: Some("snappy"),
        },
        Row {
            case: "HADOOP_SNAPPY",
            declared: Some("HADOOP_SNAPPY"),
            coding: Some("hadoop_snappy"),
        },
    ]
}

#[tokio::test]
async fn a_delivered_object_states_the_coding_its_compression_format_declares() {
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row.
    let mut legs: Vec<(String, String, Row)> = Vec::new();
    for (ordinal, row) in rows().into_iter().enumerate() {
        let stream = format!("coding-stream-{ordinal}");
        let prefix = format!("coding-{ordinal}/");
        let mut config = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
            "Prefix": prefix,
            // The shortest wait `IntervalInSeconds` admits (published range 0..900): these
            // cases are about what the object says, not about how long its buffer is held.
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        });
        if let Some(declared) = row.declared {
            config["CompressionFormat"] = json!(declared);
        }
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": config,
                }),
            )
            .await;
        legs.push((stream, prefix, row));
    }
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for (stream, _, row) in &legs {
        let (status, answered) = emulator.put_record(stream, RECORD).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must be accepted, got {answered:?}",
            row.case
        );
    }

    for (_, prefix, row) in &legs {
        let key = delivered_key(&emulator, row.case, prefix).await;
        // What a client reading the object is told, through both of the reads that report an
        // object's metadata.
        for method in ["GET", "HEAD"] {
            let answer = emulator.s3(method, &format!("/{BUCKET}/{key}")).await;
            assert_eq!(
                answer.status,
                200,
                "case {:?}: {method} {key} got {}",
                row.case,
                String::from_utf8_lossy(&answer.body)
            );
            let coding = answer.header("content-encoding");
            assert_eq!(
                coding.as_deref(),
                row.coding,
                "case {:?}: {method} must report the coding of the declared format, got \
                 {coding:?}",
                row.case
            );
            let media_type = answer.header("content-type");
            assert_eq!(
                media_type.as_deref(),
                Some(OCTET_STREAM),
                "case {:?}: {method} must report {OCTET_STREAM}, got {media_type:?}",
                row.case
            );
        }

        // And the coding is what the bytes really are: a compressed object does not hold the
        // record as the producer sent it, while an object reporting no coding does.
        let body = emulator.object(BUCKET, &key).await;
        let plain = body == RECORD;
        assert_eq!(
            plain,
            row.coding.is_none(),
            "case {:?}: the body must be encoded exactly when a coding is reported, got a \
             {}-byte body for coding {:?}",
            row.case,
            body.len(),
            row.coding
        );
    }
}

/// The one key delivered under `prefix`, or a failure naming `case` once the deadline passes.
async fn delivered_key(emulator: &Emulator, case: &str, prefix: &str) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let keys = emulator.keys_under(BUCKET, prefix).await;
        if let Some(key) = keys.into_iter().next() {
            return key;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a delivered object under {prefix:?} in {BUCKET}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}
