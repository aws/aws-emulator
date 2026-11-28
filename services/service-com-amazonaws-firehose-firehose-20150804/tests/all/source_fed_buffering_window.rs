// SPDX-License-Identifier: CC0-1.0

//! WHEN the records a Firehose stream consumed from its Kinesis data stream become objects at
//! the destination: that destination's own `BufferingHints`.
//!
//! The rule under test: a Firehose stream whose `DeliveryStreamType` is
//! `KinesisStreamAsSource` takes its data from the Kinesis data stream its
//! `KinesisStreamSourceConfiguration` names — "the stream and role Amazon Resource Names
//! (ARNs) for a Kinesis data stream used as the source for a Firehose stream" — and every
//! record that source took after retrieval started is written VERBATIM into an object under
//! the destination's `Prefix` once the first of that destination's two buffering conditions is
//! met: `IntervalInSeconds` after the buffer opened ("Buffer incoming data for the specified
//! period of time, in seconds, before delivering it to the destination") or as soon as the
//! buffer holds `SizeInMBs`. So a destination configured with a NON-ZERO interval and a size
//! the records never reach holds nothing while that window runs and holds the consumed
//! record once it has elapsed — the window being the interval it asked for while that interval
//! is inside the span this emulator holds a buffer for, and the end of that span for an
//! interval past it, the hints being hints the service may improve on — and because the source,
//! not a Firehose call, is what feeds such a stream, consumption keeps going after a delivery:
//! a record a producer writes to the source later is delivered too, and a record written twice
//! is delivered twice.
//!
//! What this suite adds to its siblings: `kinesis_source_ingestion` drives the consumption
//! path with destinations asking for the shortest interval their member admits, or for records
//! that cross `SizeInMBs`, so every row of it is due the moment its record is consumed, and
//! `buffering_hints` pins the interval-governed window on `DirectPut` streams, whose buffer
//! opens inside the put itself. Here the two meet: the window is the one a NON-ZERO
//! `IntervalInSeconds` buys, on a stream whose records arrive through its SOURCE, over
//! repeated producer writes — the shape a CLI-driven Kinesis-to-Firehose-to-Amazon-S3
//! pipeline has.
//!
//! Firehose, Amazon S3 and Kinesis Data Streams are mounted on ONE shared engine by the
//! `destination_delivery` harness, as the router mounts them, so the Firehose and Kinesis
//! calls go as `POST /` with `X-Amz-Target` and the destination is read with the path-style
//! Amazon S3 requests a client sends.

use std::time::Instant;

use serde_json::json;

use crate::buffering_hints::{LONG_INTERVAL, SHORT_INTERVAL};
use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::{carries_time_prefix, Emulator};
// The source-ingestion suite's harness, reused rather than stood up a second time: the same
// source create and wait, the same wait for a Firehose stream to be ACTIVE, the same producer
// put, the same destination reads and the same exactly-once watcher.
use crate::kinesis_source_ingestion::{
    await_active_source_fed_stream, await_source_active, concatenated, create_source, occurrences,
    put_to_source, stays_delivered, DELIVERY_DEADLINE, POLL_INTERVAL,
};

/// How many times the producer writes its payload to the source: once, and then the same bytes
/// again — the periodic re-send of a producer that keeps publishing, which is what says
/// consumption did not stop at the first delivery.
const ROUNDS: usize = 2;

/// When the destination of one row may first hold the record its source took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// At once: the row asks for the shortest interval its member admits.
    AtOnce,
    /// Only once [`SHORT_INTERVAL`] has passed.
    AfterItsInterval,
    /// Only once the span this emulator holds a buffer for has passed: its interval is minutes
    /// away and its record is far below its size, so the window it asked for is improved on at
    /// the end of that span.
    AtTheCeiling,
}

/// One case: the source's shape, the destination the Firehose stream delivers to, and when
/// that destination's own hints make a consumed record due.
struct Row {
    case: &'static str,
    /// How many shards the source Kinesis data stream is created with.
    shards: u32,
    /// The `CreateDeliveryStream` member carrying the Amazon S3 destination.
    member: &'static str,
    /// The `Prefix` the destination is configured with, `None` to leave the member out.
    prefix: Option<&'static str>,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`.
    hints: (i64, i64),
    due: Due,
}

/// The cases.
///
/// The rows differ along what the rule generalises over: the configured `IntervalInSeconds`
/// (its published minimum 0, a short non-zero one, and one minutes away — all inside the
/// published range 0..900), the configured `SizeInMBs` (1 and the published maximum 128, which
/// the one-record payloads never reach), which destination member carries the Amazon S3
/// location, the form of the `Prefix` (a single segment, a nested one, and the member left
/// out), and how many shards the source spreads its records over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a short non-zero interval, an extended destination under records/",
            shards: 1,
            member: "ExtendedS3DestinationConfiguration",
            prefix: Some("records/"),
            hints: (1, SHORT_INTERVAL),
            due: Due::AfterItsInterval,
        },
        Row {
            case: "the same interval under the largest SizeInMBs, a nested prefix, plain S3",
            shards: 2,
            member: "S3DestinationConfiguration",
            prefix: Some("records/by-day/"),
            hints: (128, SHORT_INTERVAL),
            due: Due::AfterItsInterval,
        },
        Row {
            case: "the shortest interval the member admits, no Prefix at all",
            shards: 1,
            member: "ExtendedS3DestinationConfiguration",
            prefix: None,
            hints: (1, 0),
            due: Due::AtOnce,
        },
        Row {
            case: "an interval minutes away, with a record far below its size",
            shards: 3,
            member: "ExtendedS3DestinationConfiguration",
            prefix: Some("held/"),
            hints: (128, LONG_INTERVAL),
            due: Due::AtTheCeiling,
        },
    ]
}

/// One row under test: its source stream, its Firehose stream, where it delivers and the bytes
/// its producer publishes.
struct Leg {
    case: &'static str,
    /// The source Kinesis data stream a producer writes to.
    source: String,
    /// `KinesisStreamSourceConfiguration$KinesisStreamARN`, read back off the source.
    source_arn: String,
    /// The Firehose stream consuming that source.
    stream: String,
    /// The destination bucket, one per row so a listing is per case.
    bucket: String,
    /// The `Prefix` the destination was configured with, empty when the member was left out.
    prefix: String,
    /// The payload the producer publishes, carrying a marker no other row's does.
    payload: Vec<u8>,
    due: Due,
}

#[tokio::test]
async fn a_consumed_record_is_delivered_on_its_destinations_own_buffering_window() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every source stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed span on the sources rather than one per row.
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in rows.iter().enumerate() {
        let bucket = format!("source-window-bucket-{ordinal}");
        let source = format!("source-window-source-{ordinal}");
        emulator.create_bucket(&bucket).await;
        create_source(&emulator, &source, row.shards).await;
        legs.push(Leg {
            case: row.case,
            source,
            source_arn: String::new(),
            stream: format!("source-window-stream-{ordinal}"),
            bucket,
            prefix: row.prefix.unwrap_or_default().to_string(),
            payload: payload(ordinal),
            due: row.due,
        });
    }
    for leg in &mut legs {
        leg.source_arn = await_source_active(&emulator, leg.case, &leg.source).await;
    }

    // And every Firehose stream likewise: created first, awaited after.
    for (leg, row) in legs.iter().zip(rows.iter()) {
        create_source_fed_stream(&emulator, leg, row).await;
    }
    for leg in &legs {
        await_active_source_fed_stream(&emulator, leg.case, &leg.stream, &leg.source_arn).await;
    }

    // Nothing has been published yet, so no destination holds anything: a stream with a source
    // delivers what that source took, not what its creation did.
    for leg in &legs {
        assert_copies(&emulator, leg, 0, "before the producer published anything").await;
    }

    for round in 1..=ROUNDS {
        // The producer writes to the SOURCE — a stream fed by a configured source refuses a
        // direct put, so the bytes that reach the destination can only have been consumed.
        for leg in &legs {
            put_to_source(
                &emulator,
                &leg.source,
                &format!("pk-{round}"),
                &leg.payload,
            )
            .await;
            // A destination whose window has not elapsed still holds exactly what the
            // earlier rounds delivered, read right after this round's own put.
            if leg.due != Due::AtOnce {
                assert_copies(&emulator, leg, round - 1, "right after the producer's put").await;
            }
        }
        // The destinations asking for the shortest interval their member admits are due as
        // soon as the record is consumed; the ones asking for a non-zero interval become due
        // once it has elapsed, and the one asking for minutes at the end of the span this
        // emulator holds a buffer for, which improves on the window it asked for.
        await_copies(&emulator, &legs, Due::AtOnce, round).await;
        await_copies(&emulator, &legs, Due::AfterItsInterval, round).await;
        await_copies(&emulator, &legs, Due::AtTheCeiling, round).await;
        assert_delivered_keys(&emulator, &legs).await;
    }

    // What each destination holds at the end: its payload once per round, byte for byte, and
    // nothing else — watched over a window, so a record consumed twice or delivered twice
    // fails the case whenever it happens.
    for leg in &legs {
        stays_delivered(
            &emulator,
            leg.case,
            &leg.bucket,
            &leg.prefix,
            &[(leg.payload.as_slice(), ROUNDS)],
        )
        .await;
    }
}

/// Create `leg`'s `KinesisStreamAsSource` Firehose stream, reading its source and delivering to
/// its bucket with the row's hints and prefix, without waiting for it.
async fn create_source_fed_stream(emulator: &Emulator, leg: &Leg, row: &Row) {
    let (size, interval) = row.hints;
    let mut destination = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": format!("arn:aws:s3:::{}", leg.bucket),
        "BufferingHints": { "SizeInMBs": size, "IntervalInSeconds": interval },
    });
    if let Some(prefix) = row.prefix {
        destination["Prefix"] = json!(prefix);
    }
    let mut request = json!({
        "DeliveryStreamName": &leg.stream,
        "DeliveryStreamType": "KinesisStreamAsSource",
        "KinesisStreamSourceConfiguration": {
            "KinesisStreamARN": &leg.source_arn,
            "RoleARN": ROLE_ARN,
        },
    });
    request[row.member] = destination;
    let answered = emulator.firehose_ok("CreateDeliveryStream", request).await;
    assert!(
        answered["DeliveryStreamARN"]
            .as_str()
            .is_some_and(|arn| arn.contains(&leg.stream)),
        "case {:?}: the create answers the stream's ARN, got {answered:?}",
        leg.case
    );
}

/// Require `leg`'s destination to hold its payload exactly `copies` times and nothing else.
async fn assert_copies(emulator: &Emulator, leg: &Leg, copies: usize, stage: &str) {
    let held = concatenated(emulator, &leg.bucket, &leg.prefix).await;
    let counted = occurrences(&held, &leg.payload);
    assert_eq!(
        counted, copies,
        "case {:?} ({stage}): {}/{} holds the published payload exactly {copies} time(s), got \
         {counted} in {} delivered byte(s)",
        leg.case,
        leg.bucket,
        leg.prefix,
        held.len()
    );
    assert_eq!(
        held.len(),
        leg.payload.len() * copies,
        "case {:?} ({stage}): {}/{} holds the published payload(s) and nothing else, got {:?}",
        leg.case,
        leg.bucket,
        leg.prefix,
        String::from_utf8_lossy(&held)
    );
}

/// Wait until every leg of `due` holds `copies` copies of its payload.
async fn await_copies(emulator: &Emulator, legs: &[Leg], due: Due, copies: usize) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let mut pending: Vec<(&str, usize)> = Vec::new();
        for leg in legs.iter().filter(|leg| leg.due == due) {
            let held = concatenated(emulator, &leg.bucket, &leg.prefix).await;
            let counted = occurrences(&held, &leg.payload);
            if counted != copies {
                pending.push((leg.case, counted));
            }
        }
        if pending.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {copies} delivered copy(ies) of the published payload for each {due:?} \
             case, still pending (case, copies held): {pending:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Require every delivered key of every leg to be the key a consumed record's object gets: the
/// destination's own `Prefix`, the automatic `YYYY/MM/DD/HH` expansion under it, and the name
/// of the Firehose stream that delivered it.
async fn assert_delivered_keys(emulator: &Emulator, legs: &[Leg]) {
    for leg in legs {
        for key in emulator.keys_under(&leg.bucket, &leg.prefix).await {
            assert!(
                key.starts_with(&leg.prefix),
                "case {:?}: expected a key under {:?}, got {key:?}",
                leg.case,
                leg.prefix
            );
            assert!(
                carries_time_prefix(&key, &leg.prefix),
                "case {:?}: a delivered key carries the automatic YYYY/MM/DD/HH prefix, got \
                 {key:?}",
                leg.case
            );
            assert!(
                key.contains(&leg.stream),
                "case {:?}: a delivered object is named for its Firehose stream, got {key:?}",
                leg.case
            );
        }
    }
}

/// The bytes the `ordinal`th row's producer publishes: a marker no other row carries, in the
/// shape a producer sends (a line of JSON), and short enough that no row's `SizeInMBs` is
/// reached by it.
fn payload(ordinal: usize) -> Vec<u8> {
    format!("{{\"marker\":\"source-window-{ordinal}\"}}\n").into_bytes()
}
