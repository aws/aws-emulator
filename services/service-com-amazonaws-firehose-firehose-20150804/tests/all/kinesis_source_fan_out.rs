// SPDX-License-Identifier: CC0-1.0

//! SEVERAL Firehose streams fed by ONE Kinesis data stream, driven over the wire.
//!
//! The rule under test: each Firehose stream whose source configuration names a Kinesis data
//! stream consumes that stream's shards for ITSELF — `KinesisStreamSourceConfiguration` is
//! "the stream and role Amazon Resource Names (ARNs) for a Kinesis data stream used as the
//! source for a Firehose stream", a per-stream binding, and
//! `KinesisStreamSourceDescription$DeliveryStartTimestamp` is where THAT stream "starts
//! retrieving records from the Kinesis data stream" — so how far one stream has consumed the
//! source never limits another. A record the source holds is therefore delivered by EVERY
//! stream reading it, once each, to that stream's own destination: reading a record is not
//! taking it away from the next consumer.
//!
//! The suite's rows differ along what the rule generalises over: how many streams are attached
//! to the one source, whether their destinations are separate buckets or one bucket under
//! their own prefixes, which destination member carries the Amazon S3 location, how many
//! shards the source spreads the records over, and whether the producer writes the record set
//! with `PutRecord` or with one `PutRecords`.
//!
//! Firehose, Amazon S3 and Kinesis Data Streams are mounted on ONE shared engine, as the
//! router mounts them, so the hand-off between them is what is exercised.

use serde_json::json;

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::{base64, carries_time_prefix, Emulator};
// The source-ingestion suite's harness, reused rather than stood up a second time: the same
// source stream creation, the same wait for a stream to be ACTIVE, the same producer put, the
// same destination reads, the same record markers and the same exactly-once watcher, here
// read per destination PREFIX so two streams can share one bucket.
use crate::kinesis_source_ingestion::{
    await_active_source_fed_stream, create_source_stream, delivered_payloads, once, payload,
    put_to_source, stays_delivered, KINESIS_SERVICE, KINESIS_TARGET,
};

/// One stream attached to the row's source, and where it delivers.
#[derive(Debug)]
struct Consumer {
    /// The `CreateDeliveryStream` member carrying this stream's Amazon S3 destination.
    member: &'static str,
    /// Which sink bucket of the row this stream writes to, by index — two consumers naming
    /// the same index share one bucket.
    bucket: usize,
    /// The `Prefix` the destination is configured with; empty leaves the member out.
    prefix: &'static str,
}

/// One fan-out case: the source's shape, the streams attached to it, and the record set a
/// producer writes to the source.
struct FanOutRow {
    case: &'static str,
    /// How many shards the source stream is created with, so a case spreads its records over
    /// that many shards.
    shards: u32,
    /// How many sink buckets the row creates for its consumers to index into.
    buckets: usize,
    consumers: Vec<Consumer>,
    /// How many records the producer writes to the source.
    records: usize,
    /// How many bytes of filler each record carries beyond its marker.
    record_filler: usize,
    /// Whether the record set is written with ONE `PutRecords` (true) or one `PutRecord` per
    /// record (false).
    batch: bool,
}

/// The fan-out cases.
fn fan_out_rows() -> Vec<FanOutRow> {
    vec![
        FanOutRow {
            case: "two streams on separate buckets, one shard, one PutRecord per record",
            shards: 1,
            buckets: 2,
            consumers: vec![
                Consumer {
                    member: "ExtendedS3DestinationConfiguration",
                    bucket: 0,
                    prefix: "",
                },
                Consumer {
                    member: "S3DestinationConfiguration",
                    bucket: 1,
                    prefix: "curated/",
                },
            ],
            records: 3,
            record_filler: 0,
            batch: false,
        },
        FanOutRow {
            case: "two streams sharing one bucket under their own prefixes, one PutRecords",
            shards: 2,
            buckets: 1,
            consumers: vec![
                Consumer {
                    member: "S3DestinationConfiguration",
                    bucket: 0,
                    prefix: "raw/",
                },
                Consumer {
                    member: "ExtendedS3DestinationConfiguration",
                    bucket: 0,
                    prefix: "converted/nested/",
                },
            ],
            records: 5,
            record_filler: 48,
            batch: true,
        },
        FanOutRow {
            case: "three streams on separate buckets, four shards, one PutRecords",
            shards: 4,
            buckets: 3,
            consumers: vec![
                Consumer {
                    member: "ExtendedS3DestinationConfiguration",
                    bucket: 0,
                    prefix: "",
                },
                Consumer {
                    member: "S3DestinationConfiguration",
                    bucket: 1,
                    prefix: "archive/",
                },
                Consumer {
                    member: "ExtendedS3DestinationConfiguration",
                    bucket: 2,
                    prefix: "table/",
                },
            ],
            records: 6,
            record_filler: 16,
            batch: true,
        },
    ]
}

#[tokio::test]
async fn every_stream_reading_one_source_delivers_every_record() {
    for (ordinal, row) in fan_out_rows().into_iter().enumerate() {
        let emulator = Emulator::new();
        let buckets: Vec<String> = (0..row.buckets)
            .map(|index| format!("fan-out-bucket-{ordinal}-{index}"))
            .collect();
        for bucket in &buckets {
            emulator.create_bucket(bucket).await;
        }
        let source = format!("fan-out-source-{ordinal}");
        let source_arn = create_source_stream(&emulator, &source, row.shards).await;

        // Every stream is attached, and ACTIVE, BEFORE the producer writes: each one's
        // retrieval therefore starts before the record set exists, so the whole set is its
        // data.
        let mut streams = Vec::new();
        for (index, consumer) in row.consumers.iter().enumerate() {
            let name = format!("fan-out-stream-{ordinal}-{index}");
            attach_consumer(
                &emulator,
                row.case,
                &name,
                &source_arn,
                consumer.member,
                consumer.prefix,
                &buckets[consumer.bucket],
            )
            .await;
            streams.push(name);
        }

        for consumer in &row.consumers {
            let idle = emulator
                .keys_under(&buckets[consumer.bucket], consumer.prefix)
                .await;
            assert!(
                idle.is_empty(),
                "case {:?}: an idle source delivers nothing under {:?}, got {idle:?}",
                row.case,
                consumer.prefix
            );
        }

        // What the producer writes to the SOURCE, once — never once per consumer.
        let payloads: Vec<Vec<u8>> = (0..row.records)
            .map(|index| payload(ordinal, index, row.record_filler))
            .collect();
        if row.batch {
            put_records_to_source(&emulator, row.case, &source, &payloads).await;
        } else {
            for (index, payload) in payloads.iter().enumerate() {
                put_to_source(&emulator, &source, &format!("pk-{index}"), payload).await;
            }
        }

        // Each attached stream delivers the WHOLE record set to its own destination.
        for (consumer, stream) in row.consumers.iter().zip(&streams) {
            let bucket = &buckets[consumer.bucket];
            let label = labelled(row.case, stream);
            let delivered =
                delivered_payloads(&emulator, &label, bucket, consumer.prefix, &payloads).await;
            for key in delivered.keys() {
                assert!(
                    key.starts_with(consumer.prefix),
                    "case {label:?}: expected a key under {:?}, got {key:?}",
                    consumer.prefix
                );
                assert!(
                    carries_time_prefix(key, consumer.prefix),
                    "case {label:?}: a delivered key carries the automatic YYYY/MM/DD/HH \
                     prefix, got {key:?}"
                );
                assert!(
                    key.contains(stream),
                    "case {label:?}: an object delivered by {stream} is named for it, \
                     got {key:?}"
                );
            }
        }

        // And each delivers it ONCE: another stream's consumption neither duplicates a
        // record nor takes one away.
        for (consumer, stream) in row.consumers.iter().zip(&streams) {
            stays_delivered(
                &emulator,
                &labelled(row.case, stream),
                &buckets[consumer.bucket],
                consumer.prefix,
                &once(&payloads),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_stream_attached_later_reads_from_its_own_start() {
    let emulator = Emulator::new();
    let case = "a stream attached later reads from its own start";
    let early_bucket = "later-attach-bucket-early".to_string();
    let late_bucket = "later-attach-bucket-late".to_string();
    emulator.create_bucket(&early_bucket).await;
    emulator.create_bucket(&late_bucket).await;
    let source = "later-attach-source".to_string();
    let source_arn = create_source_stream(&emulator, &source, 1).await;

    let first = "later-attach-stream-first".to_string();
    attach_consumer(
        &emulator,
        case,
        &first,
        &source_arn,
        "ExtendedS3DestinationConfiguration",
        "",
        &early_bucket,
    )
    .await;

    // A record the source takes while only the first stream is attached, delivered before
    // the second stream exists.
    let early = b"written-while-one-stream-was-attached".to_vec();
    put_to_source(&emulator, &source, "pk-early", &early).await;
    delivered_payloads(
        &emulator,
        &labelled(case, &first),
        &early_bucket,
        "",
        std::slice::from_ref(&early),
    )
    .await;

    let second = "later-attach-stream-second".to_string();
    attach_consumer(
        &emulator,
        case,
        &second,
        &source_arn,
        "S3DestinationConfiguration",
        "late/",
        &late_bucket,
    )
    .await;

    let late = b"written-while-both-streams-were-attached".to_vec();
    put_to_source(&emulator, &source, "pk-late", &late).await;

    // The record written after both are attached reaches BOTH destinations.
    delivered_payloads(
        &emulator,
        &labelled(case, &first),
        &early_bucket,
        "",
        std::slice::from_ref(&late),
    )
    .await;
    delivered_payloads(
        &emulator,
        &labelled(case, &second),
        &late_bucket,
        "late/",
        std::slice::from_ref(&late),
    )
    .await;

    // The earlier record is not the later stream's data: its retrieval starts at its OWN
    // `DeliveryStartTimestamp`, and it stays there — the later stream's destination holds the
    // record written after it was attached, exactly once, and nothing else.
    stays_delivered(
        &emulator,
        &labelled(case, &second),
        &late_bucket,
        "late/",
        &[(late.as_slice(), 1), (early.as_slice(), 0)],
    )
    .await;
    // The first stream, which WAS attached then, holds both exactly once.
    stays_delivered(
        &emulator,
        &labelled(case, &first),
        &early_bucket,
        "",
        &[(early.as_slice(), 1), (late.as_slice(), 1)],
    )
    .await;
}

/// The case `case` as observed through the stream `stream`: what an assertion of this suite
/// names, since every row drives several streams through the same case.
fn labelled(case: &str, stream: &str) -> String {
    format!("{case} ({stream})")
}

/// Create the `KinesisStreamAsSource` Firehose stream `name` reading `source_arn` and
/// delivering to `bucket` under `prefix`, and wait for it to report `ACTIVE`.
async fn attach_consumer(
    emulator: &Emulator,
    case: &str,
    name: &str,
    source_arn: &str,
    member: &str,
    prefix: &str,
    bucket: &str,
) {
    let mut destination = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": format!("arn:aws:s3:::{bucket}"),
        // The smallest size and the shortest interval the member admits, so a case waits for
        // the delivery path and not for a buffer window it did not ask to test.
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
    });
    if !prefix.is_empty() {
        destination["Prefix"] = json!(prefix);
    }
    let mut request = json!({
        "DeliveryStreamName": name,
        "DeliveryStreamType": "KinesisStreamAsSource",
        "KinesisStreamSourceConfiguration": {
            "KinesisStreamARN": source_arn,
            "RoleARN": ROLE_ARN,
        },
    });
    request[member] = destination;
    let answered = emulator.firehose_ok("CreateDeliveryStream", request).await;
    assert!(
        answered["DeliveryStreamARN"]
            .as_str()
            .is_some_and(|arn| arn.contains(name)),
        "case {case:?}: the create answers {name}'s ARN, got {answered:?}"
    );
    await_active_source_fed_stream(emulator, case, name, source_arn).await;
}

/// Write the whole record set to the source with ONE `PutRecords`, as a batching producer
/// does.
async fn put_records_to_source(
    emulator: &Emulator,
    case: &str,
    source: &str,
    payloads: &[Vec<u8>],
) {
    let entries: Vec<_> = payloads
        .iter()
        .enumerate()
        .map(|(index, payload)| {
            json!({ "PartitionKey": format!("pk-{index}"), "Data": base64(payload) })
        })
        .collect();
    let (status, answered) = emulator
        .json_rpc(
            KINESIS_SERVICE,
            KINESIS_TARGET,
            "PutRecords",
            json!({ "StreamName": source, "Records": entries }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRecords to the source {source}, got {answered:?}"
    );
    let failed = answered["FailedRecordCount"].as_i64().unwrap_or(0);
    assert_eq!(
        failed, 0,
        "case {case:?}: the source takes every record of the batch, got {answered:?}"
    );
    let stored = answered["Records"].as_array().map_or(0, Vec::len);
    assert_eq!(
        stored,
        payloads.len(),
        "case {case:?}: the source answers one result per record, got {answered:?}"
    );
}
