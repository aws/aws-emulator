// SPDX-License-Identifier: CC0-1.0

//! WHEN a stream's buffer becomes an Amazon S3 object: the destination's `BufferingHints`.
//!
//! The rule under test: Firehose holds the records a put accepted and delivers them once the
//! FIRST of the destination's two buffering conditions is met — `IntervalInSeconds` after the
//! buffer opened ("Buffer incoming data for the specified period of time, in seconds, before
//! delivering it to the destination") or as soon as the buffer holds `SizeInMBs` ("Buffer
//! incoming data to the specified size, in MiBs, before delivering it to the destination"),
//! with the documented defaults (300 s, 5 MiB) standing where the configuration names
//! neither. Each flushed buffer becomes ONE object. The hints that decide a window are the
//! ones of the destination member the stream was created with, whichever member carries the
//! Amazon S3 location every record passes through: the plain `S3DestinationConfiguration`,
//! the extended one, or the `S3Configuration` of an Amazon Redshift destination ("Firehose
//! first delivers data to the S3 bucket and then issues an Amazon Redshift COPY command").
//!
//! So a destination's bucket holds NOTHING for a record put inside a longer interval, holds
//! one object for the same record once that interval has passed, and holds one as soon as the
//! records cross the configured size however long the interval is.
//!
//! Every row asserts that window rather than any particular latency, and no case waits for a
//! fixed time or reads the wall clock: a destination is read right after its own put, a
//! destination that IS due is polled until it holds its object, and the destinations whose
//! interval is minutes away are required to hold nothing at every reading taken while that
//! polling runs.
//!
//! The rows here vary the HINT VALUES of one destination member; the sibling suite
//! `buffering_hints_destination_members` varies the MEMBER carrying them, and drives its own
//! rows through [`assert_window_rule`] below, so there is one table-driver and one reading of
//! "delivered" and "not delivered yet" between the two. The windows these rows wait out are
//! the ones a case can; that a window of MINUTES is waited out too rather than quietly
//! shortened is the child suite [`hint_ceiling`].

/// What a destination asking for an interval at the CEILING of the published range gets. The
/// `#[path]` is what keeps it a child of THIS suite rather than a second suite
/// beside it: this crate's `tests/all.rs` declares the suites directly under `tests/all/`, and
/// a module of a `#[path]`-declared file resolves next to that file unless it says otherwise.
#[path = "buffering_hints/hint_ceiling.rs"]
mod hint_ceiling;

/// What the destination's own `ListObjectsV2` reports about the object a delivered buffer
/// became, for each way the buffering hints can be spelled. A child of THIS suite for the same
/// reason as the module above.
#[path = "buffering_hints/delivered_listing_entry.rs"]
mod delivered_listing_entry;

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::Emulator;

/// The bucket every case here delivers into.
const BUCKET: &str = "buffering-hints-bucket";

/// The interval the rows that wait one out are configured with, in seconds.
///
/// Inside the published range of `IntervalInSeconds` (0..900), and short enough that a case
/// waiting one out is not waiting on [`DELIVERY_DEADLINE`].
pub(crate) const SHORT_INTERVAL: i64 = 5;

/// A minutes-long interval, in seconds: inside the same published range, and far longer than
/// every deadline here, so a buffer configured with it must hold nothing at every reading a
/// case takes.
pub(crate) const LONG_INTERVAL: i64 = 120;

/// How long a stream is polled for `ACTIVE` before a case fails: far longer than the
/// emulator's `CREATING` span, so a slow machine does not fail a case the clock would have
/// served.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a delivery that IS due is waited for before a case fails, so a slow machine does
/// not fail a case the clock would have served. Generous next to [`SHORT_INTERVAL`] and far
/// inside [`LONG_INTERVAL`], so a buffer that must still be held is still held at every
/// reading taken inside this window.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(30);

/// How often a destination, or a stream's status, is polled while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A record big enough that a few of them cross one MiB while each stays inside the
/// per-record limit of `Record$Data`.
pub(crate) const BIG_RECORD_BYTES: usize = 600 * 1024;

/// A record big enough that six of them cross the documented default `SizeInMBs` of 5 MiB.
const HUGE_RECORD_BYTES: usize = 900 * 1024;

/// Which destination member of `CreateDeliveryStreamInput` carries the Amazon S3 location
/// whose `BufferingHints` are under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Member {
    /// `S3DestinationConfiguration`, the plain Amazon S3 destination.
    PlainS3,
    /// `ExtendedS3DestinationConfiguration`.
    ExtendedS3,
    /// `RedshiftDestinationConfiguration$S3Configuration`, which every record of an Amazon
    /// Redshift destination passes through on its way to the cluster.
    RedshiftS3,
}

impl Member {
    /// The `CreateDeliveryStreamInput` member this destination is sent as.
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Member::PlainS3 => "S3DestinationConfiguration",
            Member::ExtendedS3 => "ExtendedS3DestinationConfiguration",
            Member::RedshiftS3 => "RedshiftDestinationConfiguration",
        }
    }
}

/// When the destination of one row may first hold an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Due {
    /// At once: the configured interval is the published minimum, or the records put cross
    /// `SizeInMBs` so the size condition is the one that fires.
    AtOnce,
    /// Only once [`SHORT_INTERVAL`] has passed.
    AfterItsInterval,
    /// Not while this case runs: its interval is minutes away and its records never reach
    /// its size.
    NotYet,
}

/// How the row's records are put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Put {
    /// One `PutRecordBatch` carrying every record.
    OneBatch,
    /// One `PutRecord` per record.
    OneCallEach,
}

/// One buffering case.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    /// Which destination member carries this row's Amazon S3 location and its hints.
    pub(crate) member: Member,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`; `None` leaves the member out
    /// altogether, which is the documented defaults.
    pub(crate) hints: Option<(i64, i64)>,
    /// How many records are put, and how many bytes of filler each carries.
    pub(crate) records: (usize, usize),
    pub(crate) put: Put,
    pub(crate) due: Due,
}

/// The buffering cases.
///
/// The rows differ along what the rule generalises over: the configured `IntervalInSeconds`
/// (its published minimum 0, a short one, its published maximum 900, and the member left out
/// for the documented default of 300), the configured `SizeInMBs` (1, the published maximum
/// 128, and the member left out for the documented default of 5), whether the records put
/// CROSS that size, how many records there are and how big each is, and whether they are put
/// in one batch or one call each. The MEMBER carrying the hints is varied by the sibling
/// suite's rows, which this suite's driver serves too.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest interval the member admits, with a tiny record",
            member: Member::ExtendedS3,
            hints: Some((1, 0)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::AtOnce,
        },
        Row {
            case: "a short interval, with a payload far below SizeInMBs",
            member: Member::ExtendedS3,
            hints: Some((1, SHORT_INTERVAL)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "the same short interval under the largest SizeInMBs the member admits",
            member: Member::ExtendedS3,
            hints: Some((128, SHORT_INTERVAL)),
            records: (3, 0),
            put: Put::OneCallEach,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "no BufferingHints at all, so the documented 300 s default interval stands",
            member: Member::ExtendedS3,
            hints: None,
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::NotYet,
        },
        Row {
            case: "the longest interval the member admits, with a tiny record",
            member: Member::ExtendedS3,
            hints: Some((1, 900)),
            records: (1, 0),
            put: Put::OneBatch,
            due: Due::NotYet,
        },
        Row {
            case: "the longest interval with the largest size, and records far below it",
            member: Member::ExtendedS3,
            hints: Some((128, 900)),
            records: (2, BIG_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::NotYet,
        },
        Row {
            case: "records crossing SizeInMBs=1 inside the longest interval",
            member: Member::ExtendedS3,
            hints: Some((1, 900)),
            records: (2, BIG_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
        Row {
            case: "records crossing the documented default SizeInMBs with no hints at all",
            member: Member::ExtendedS3,
            hints: None,
            records: (6, HUGE_RECORD_BYTES),
            put: Put::OneCallEach,
            due: Due::AtOnce,
        },
    ]
}

/// One row's stream under test, with the records it put and where they land.
struct Leg {
    case: &'static str,
    stream: String,
    bucket: String,
    /// The `Prefix` the row's destination is configured with, which isolates its objects.
    prefix: String,
    /// The records put, in arrival order.
    records: Vec<Vec<u8>>,
    put: Put,
    due: Due,
}

impl Leg {
    /// This leg as a destination under watch.
    fn watch(&self) -> Watch<'_> {
        Watch {
            case: self.case,
            bucket: &self.bucket,
            prefix: &self.prefix,
            records: &self.records,
        }
    }
}

#[tokio::test]
async fn a_buffer_is_delivered_on_the_first_buffering_hint_it_meets_and_not_before() {
    assert_window_rule(BUCKET, "buffering-stream", rows()).await;
}

/// Require every row of `rows` to be delivered on the first of its destination's own
/// buffering conditions and not before: each row gets its own stream named after
/// `stream_prefix` and its own `Prefix` under `bucket`, so the readings are per case.
///
/// `pub(crate)` because the sibling suite varying the destination MEMBER asserts the same
/// rule over its own rows — one driver, one reading of "delivered" and "not delivered yet".
pub(crate) async fn assert_window_rule(bucket: &str, stream_prefix: &str, rows: Vec<Row>) {
    let emulator = Emulator::new();
    emulator.create_bucket(bucket).await;

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row.
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in rows.into_iter().enumerate() {
        let stream = format!("{stream_prefix}-{ordinal}");
        let prefix = format!("row-{ordinal}/");
        let mut request = json!({
            "DeliveryStreamName": &stream,
            "DeliveryStreamType": "DirectPut",
        });
        request[row.member.wire()] = destination(row.member, bucket, &prefix, row.hints);
        emulator.firehose_ok("CreateDeliveryStream", request).await;
        let (count, filler) = row.records;
        legs.push(Leg {
            case: row.case,
            stream,
            bucket: bucket.to_string(),
            prefix,
            records: (0..count)
                .map(|index| payload(ordinal, index, filler))
                .collect(),
            put: row.put,
            due: row.due,
        });
    }
    for leg in &legs {
        await_active(&emulator, leg.case, &leg.stream).await;
    }

    // Each leg is read right after its OWN put, so what the reading says about its window
    // does not depend on how long the other legs took: a put does not deliver what it
    // accepted unless the put itself met one of the destination's conditions.
    for leg in &legs {
        match leg.put {
            Put::OneBatch => {
                let (status, answered) = emulator.put_batch(&leg.stream, &leg.records).await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the batch must be accepted, got {answered:?}",
                    leg.case
                );
            }
            Put::OneCallEach => {
                for record in &leg.records {
                    let (status, answered) = emulator.put_record(&leg.stream, record).await;
                    assert_eq!(
                        status, 200,
                        "case {:?}: the put must be accepted, got {answered:?}",
                        leg.case
                    );
                }
            }
        }
        if leg.due != Due::AtOnce {
            assert_nothing_delivered(&emulator, &leg.watch()).await;
        }
    }

    // The buffers whose size condition has fired, and the ones configured at the shortest
    // interval their member admits, are objects now. The ones whose interval is minutes away
    // must hold nothing at every reading taken while those are waited for.
    let held: Vec<Watch<'_>> = watches(&legs, Due::NotYet);
    await_delivered(&emulator, &watches(&legs, Due::AtOnce), &held).await;

    // And the buffers configured with the short interval become objects once it has elapsed,
    // while the minutes-long ones, read all through that wait, still hold nothing.
    await_delivered(&emulator, &watches(&legs, Due::AfterItsInterval), &held).await;
}

/// Every leg whose destination is due at `due`, as destinations under watch.
fn watches(legs: &[Leg], due: Due) -> Vec<Watch<'_>> {
    legs.iter()
        .filter(|leg| leg.due == due)
        .map(Leg::watch)
        .collect()
}

/// The destination configuration one row is created with: an Amazon S3 location in `bucket`
/// under `prefix`, carrying `hints`, sent as whatever `member` asks for.
fn destination(member: Member, bucket: &str, prefix: &str, hints: Option<(i64, i64)>) -> Value {
    let mut s3 = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": format!("arn:aws:s3:::{bucket}"),
        "Prefix": prefix,
        "CompressionFormat": "UNCOMPRESSED",
    });
    if let Some((size, interval)) = hints {
        s3["BufferingHints"] = json!({
            "SizeInMBs": size,
            "IntervalInSeconds": interval,
        });
    }
    match member {
        Member::PlainS3 | Member::ExtendedS3 => s3,
        // An Amazon Redshift destination addresses a cluster, and its required members are
        // what makes the configuration admissible; the location under test is its
        // `S3Configuration`.
        Member::RedshiftS3 => json!({
            "RoleARN": ROLE_ARN,
            "ClusterJDBCURL":
                "jdbc:redshift://warehouse.123456789012.us-west-2.redshift.amazonaws.com:5439/dev",
            "CopyCommand": { "DataTableName": "events" },
            "Username": "loader",
            "Password": "loader-secret",
            "S3Configuration": s3,
        }),
    }
}

/// One destination under watch: where its objects land, and what its buffer holds.
struct Watch<'a> {
    /// The case this destination belongs to, named by every assertion about it.
    case: &'a str,
    bucket: &'a str,
    /// The `Prefix` the destination is configured with, which isolates its objects.
    prefix: &'a str,
    /// The records its buffer holds, in arrival order.
    records: &'a [Vec<u8>],
}

/// Wait until `stream` reports `ACTIVE`, which is when a put to it is accepted.
async fn await_active(emulator: &Emulator, case: &str, stream: &str) {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let described = emulator
            .firehose_ok(
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": stream }),
            )
            .await;
        let status = described["DeliveryStreamDescription"]["DeliveryStreamStatus"].clone();
        if status == json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stream to reach ACTIVE, last saw {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Require `watch`'s destination to hold nothing: its buffer's own conditions are not met.
async fn assert_nothing_delivered(emulator: &Emulator, watch: &Watch<'_>) {
    let keys = emulator.keys_under(watch.bucket, watch.prefix).await;
    assert!(
        keys.is_empty(),
        "case {:?}: nothing may be delivered before this destination's own hints are met, \
         got {keys:?}",
        watch.case
    );
}

/// Wait until every destination of `due` holds its delivered object, requiring every
/// destination of `held` to hold NOTHING at each reading taken meanwhile.
async fn await_delivered(emulator: &Emulator, due: &[Watch<'_>], held: &[Watch<'_>]) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        for watch in held {
            let keys = emulator.keys_under(watch.bucket, watch.prefix).await;
            assert!(
                keys.is_empty(),
                "case {:?}: a buffer whose interval has not elapsed is not delivered, \
                 got {keys:?}",
                watch.case
            );
        }
        let mut pending: Vec<&str> = Vec::new();
        for watch in due {
            if emulator
                .keys_under(watch.bucket, watch.prefix)
                .await
                .is_empty()
            {
                pending.push(watch.case);
            }
        }
        if pending.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected a delivered object for each of {pending:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    for watch in due {
        assert_one_object_holding(emulator, watch).await;
    }
}

/// Require `watch`'s destination to hold exactly one object, carrying every record of the
/// flushed buffer in arrival order — "one object per flushed batch".
async fn assert_one_object_holding(emulator: &Emulator, watch: &Watch<'_>) {
    let keys = emulator.keys_under(watch.bucket, watch.prefix).await;
    assert_eq!(
        keys.len(),
        1,
        "case {:?}: one flushed buffer is one object, got {keys:?}",
        watch.case
    );
    let body = emulator.object(watch.bucket, &keys[0]).await;
    let expected = watch.records.concat();
    assert_eq!(
        body.len(),
        expected.len(),
        "case {:?}: the object holds every record of the buffer ({} bytes), got {} bytes",
        watch.case,
        expected.len(),
        body.len()
    );
    assert!(
        body == expected,
        "case {:?}: the object holds the buffer's records in arrival order",
        watch.case
    );
}

/// The bytes of one record: a marker naming where it came from, padded with `filler` bytes.
fn payload(row: usize, record: usize, filler: usize) -> Vec<u8> {
    let mut data = format!("row-{row}-record-{record}-").into_bytes();
    data.extend(std::iter::repeat_n(b'x', filler));
    data
}
