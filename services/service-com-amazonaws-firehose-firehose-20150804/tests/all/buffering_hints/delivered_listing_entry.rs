// SPDX-License-Identifier: CC0-1.0

//! What the destination bucket's own `ListObjectsV2` reports once a stream's buffer has been
//! delivered — the ENTRY, not just the key.
//!
//! The rule under test: the records a put accepted into a `DirectPut` stream become exactly one
//! object at the Amazon S3 location the destination names, within the span this emulator holds
//! data for, whatever `BufferingHints` the destination carries, however those hints are SPELLED
//! — "these options are treated as hints, and therefore Firehose might choose to use different
//! values when it is optimal", with "the default value is 5" / "the default value is 300", so
//! the documented defaults stated explicitly are the same configuration as the member left out,
//! and an interval longer than that span is improved on rather than waited out — and whatever
//! has since happened to the stream's TAGS, because `TagDeliveryStream` and
//! `UntagDeliveryStream` say nothing about delivery. Nothing a put accepted is therefore held
//! forever. The delivered object is then an ordinary object of that bucket, so the bucket's
//! `ListObjectsV2` DESCRIBES it: it is counted in `KeyCount`, the page is not truncated, and
//! one `Contents` entry carries its `Key` under the automatic `YYYY/MM/DD/HH` prefix, its
//! `Size` (the bytes the buffer delivered), its `ETag`, its `LastModified`, the `StorageClass`
//! a write naming none is stored under, and the checksum members the delivering write recorded.
//! That listing is how a producer writing through Firehose verifies its own delivery, so it is
//! read here exactly as such a client reads it: `GET /<bucket>?list-type=2` over the
//! destination service's own wire.
//!
//! The sibling suite [`super::hint_ceiling`] pins the other half of the same rule — an interval
//! of minutes is not collapsed into the put that opened the buffer, so its rows hold their
//! records all through the readings taken well inside the span — and the parent suite
//! [`crate::buffering_hints`] that an interval inside the span is honoured to the second. This
//! one pins the END of the window: that every one of those configurations is delivered, and
//! WHAT the destination's listing then says about the object, which is the only thing such a
//! client can see. The destination builder, the wait for `ACTIVE`, the tag mutation and the
//! listing read are those suites' own, so there is one of each between them.
//!
//! No case here sleeps a window out or reads the wall clock: the destinations whose records are
//! held are read right after their own put, where they must hold nothing, and then polled until
//! the listing describes their object.

use std::time::{Duration, Instant};

use super::hint_ceiling::{change_and_remove_tags, Tagging};
use super::{await_active, destination, Member};
use crate::destination_delivery::{carries_time_prefix, Emulator};

/// How long a row's delivery is waited for before the case fails.
///
/// Generous beside the span this emulator holds data for, so a loaded machine does not fail a
/// case the clock would have served; it bounds nothing the case measures.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(240);

/// How often a destination is polled while waiting for its object.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `StorageClass` an object written with no storage class of its own is stored under, which
/// is what the delivering write names (`Object$StorageClass`, whose value set carries it).
const DEFAULT_STORAGE_CLASS: &str = "STANDARD";

/// The `ChecksumAlgorithm` the delivering write records, and the type it carries: a whole
/// object written by a `PutObject` that names no algorithm of its own
/// (`Object$ChecksumAlgorithm`, `Object$ChecksumType`).
const DELIVERED_CHECKSUM: (&str, &str) = ("CRC32", "FULL_OBJECT");

/// One case: how the destination's `BufferingHints` are spelled, what happens to the stream's
/// tags, and what its producer puts.
struct Row {
    case: &'static str,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`; `None` leaves the member out
    /// altogether, which is the documented defaults.
    hints: Option<(i64, i64)>,
    tagging: Tagging,
    /// The records put, one `PutRecord` each, in arrival order.
    records: &'static [&'static str],
}

/// The cases.
///
/// The rows differ along what the rule generalises over: how the `BufferingHints` are spelled
/// (the documented defaults stated explicitly, the member left out so those defaults stand
/// unstated, and each published bound stated explicitly), what the stream's tags were made to
/// do beforehand (nothing, or changed and then removed), and how many records of what lengths
/// one buffer holds — which is what the entry's `Size` is read against. The published ranges
/// are `SizeInMBs` 1..128 and `IntervalInSeconds` 0..900, and the defaults are `BufferingHints`'
/// own, "The default value is 5" and "The default value is 300"
/// (`smithy_query(query="shape", shape="BufferingHints,SizeInMBs,IntervalInSeconds")`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the documented defaults stated explicitly, with the tags changed and then \
                   removed",
            hints: Some((5, 300)),
            tagging: Tagging::ChangedAndRemoved,
            records: &["one record a producer put"],
        },
        Row {
            case: "the same defaults stated explicitly, never tagged, two records of \
                   different lengths",
            hints: Some((5, 300)),
            tagging: Tagging::Untouched,
            records: &["short", "a considerably longer record than the first"],
        },
        Row {
            case: "no BufferingHints member at all, with a tag added and then removed",
            hints: None,
            tagging: Tagging::ChangedAndRemoved,
            records: &["the defaults left unstated"],
        },
        Row {
            case: "the published maxima of both hints, stated explicitly",
            hints: Some((128, 900)),
            tagging: Tagging::Untouched,
            records: &["the longest window the member admits"],
        },
        Row {
            case: "the published minima of both hints, with the tags changed and then removed",
            hints: Some((1, 0)),
            tagging: Tagging::ChangedAndRemoved,
            records: &["a", "bb", "ccc"],
        },
    ]
}

/// Whether a row's records are HELD past its own put: a destination asking for any interval at
/// all is still inside its window when the put returns, while one asking for the shortest
/// interval the member admits is not. Read off the row's `hints` — the member left out is the
/// documented default interval — so a row added to the table cannot disagree with the window
/// rule.
fn held(hints: Option<(i64, i64)>) -> bool {
    hints.is_none_or(|(_, interval)| interval > 0)
}

/// One row under test: its stream, the bucket it delivers into, and what its producer put.
struct Leg {
    case: &'static str,
    stream: String,
    bucket: String,
    /// The `Prefix` its destination is configured with, which every key it delivers carries.
    prefix: String,
    /// The records put, in arrival order.
    records: Vec<Vec<u8>>,
    tagging: Tagging,
    held: bool,
}

impl Leg {
    /// The bytes the delivered object carries: the buffer's records in arrival order, which is
    /// what an `UNCOMPRESSED` destination with no processing writes.
    fn delivered(&self) -> Vec<u8> {
        self.records.concat()
    }
}

#[tokio::test]
async fn the_destination_listing_describes_the_object_a_delivered_buffer_became() {
    let emulator = Emulator::new();

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row. Each row delivers into its
    // OWN bucket, so the whole-bucket listing a producer takes is per case.
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in rows().into_iter().enumerate() {
        let bucket = format!("listing-entry-bucket-{ordinal}");
        let stream = format!("listing-entry-stream-{ordinal}");
        let prefix = format!("row-{ordinal}/");
        emulator.create_bucket(&bucket).await;
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                serde_json::json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration":
                        destination(Member::ExtendedS3, &bucket, &prefix, row.hints),
                }),
            )
            .await;
        legs.push(Leg {
            case: row.case,
            stream,
            bucket,
            prefix,
            records: row
                .records
                .iter()
                .map(|record| record.as_bytes().to_vec())
                .collect(),
            tagging: row.tagging,
            held: held(row.hints),
        });
    }
    for leg in &legs {
        await_active(&emulator, leg.case, &leg.stream).await;
    }

    // The tag mutations, before anything is put: a stream whose tags were changed and then
    // removed is the same stream as far as its destination is concerned.
    for leg in legs
        .iter()
        .filter(|leg| leg.tagging == Tagging::ChangedAndRemoved)
    {
        change_and_remove_tags(&emulator, leg.case, &leg.stream).await;
    }

    // The puts. A put is the only Firehose call a producer makes, so what follows is what the
    // emulator's own delivery clock did with the records these accepted.
    for leg in &legs {
        for record in &leg.records {
            let (status, answered) = emulator.put_record(&leg.stream, record).await;
            assert_eq!(
                status, 200,
                "case {:?}: the put must be accepted, got {answered:?}",
                leg.case
            );
        }
        if leg.held {
            let listed = emulator.listing_elements(&leg.bucket).await;
            assert!(
                values(&listed, KEY).is_empty(),
                "case {:?}: a buffer still inside its window holds its records, the listing \
                 reports {listed:?}",
                leg.case
            );
            assert_eq!(
                single(leg.case, &listed, KEY_COUNT),
                "0",
                "case {:?}: an empty destination counts no key, got {listed:?}",
                leg.case
            );
        }
    }

    for leg in &legs {
        let listed = await_listed_object(&emulator, leg).await;
        assert_entry(&emulator, leg, &listed).await;
    }
}

/// The `Contents` members and page counters this suite reads, as the elements of the
/// `ListBucketResult` body carry them.
const KEY: &str = "Key";
const KEY_COUNT: &str = "KeyCount";
const IS_TRUNCATED: &str = "IsTruncated";
const SIZE: &str = "Size";
const STORAGE_CLASS: &str = "StorageClass";
const CHECKSUM_ALGORITHM: &str = "ChecksumAlgorithm";
const CHECKSUM_TYPE: &str = "ChecksumType";
const E_TAG: &str = "ETag";
const LAST_MODIFIED: &str = "LastModified";

/// The text of every `name` element of a listing, in document order.
fn values(listed: &[(String, String)], name: &str) -> Vec<String> {
    listed
        .iter()
        .filter(|(element, _)| element == name)
        .map(|(_, text)| text.clone())
        .collect()
}

/// The text of the ONE `name` element of a listing — a page that reports a second one fails
/// the case, which is what says these members all describe the same single entry.
fn single(case: &str, listed: &[(String, String)], name: &str) -> String {
    let read = values(listed, name);
    assert_eq!(
        read.len(),
        1,
        "case {case:?}: expected exactly one {name} element in the listing, got {read:?}"
    );
    read.into_iter().next().unwrap_or_default()
}

/// Poll `leg`'s destination until its listing describes exactly one object, answering that
/// listing, or fail naming the case once [`DELIVERY_DEADLINE`] has passed.
async fn await_listed_object(emulator: &Emulator, leg: &Leg) -> Vec<(String, String)> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let listed = emulator.listing_elements(&leg.bucket).await;
        if !values(&listed, KEY).is_empty() {
            assert_eq!(
                single(leg.case, &listed, KEY_COUNT),
                "1",
                "case {:?}: one flushed buffer is one object, and the page counts the keys it \
                 lists, got {listed:?}",
                leg.case
            );
            assert_eq!(
                single(leg.case, &listed, IS_TRUNCATED),
                "false",
                "case {:?}: a page holding every key is not truncated, got {listed:?}",
                leg.case
            );
            return listed;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected the records put to be delivered into {}, its listing holds \
             nothing",
            leg.case,
            leg.bucket
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Require `listed` to describe the object `leg`'s buffer became, and the object itself to read
/// back as what that entry says it is.
async fn assert_entry(emulator: &Emulator, leg: &Leg, listed: &[(String, String)]) {
    let member = |name: &str| single(leg.case, listed, name);
    let key = member(KEY);
    assert!(
        carries_time_prefix(&key, &leg.prefix),
        "case {:?}: expected a key under {:?} and the automatic YYYY/MM/DD/HH prefix, got \
         {key:?}",
        leg.case,
        leg.prefix
    );
    let delivered = leg.delivered();
    assert_eq!(
        member(SIZE),
        delivered.len().to_string(),
        "case {:?}: the entry sizes the {} record(s) delivered",
        leg.case,
        leg.records.len()
    );
    assert_eq!(
        member(STORAGE_CLASS),
        DEFAULT_STORAGE_CLASS,
        "case {:?}: expected the storage class a write naming none is stored under",
        leg.case
    );
    let (algorithm, checksum_type) = DELIVERED_CHECKSUM;
    assert_eq!(
        member(CHECKSUM_ALGORITHM),
        algorithm,
        "case {:?}: expected the checksum algorithm the delivering write records",
        leg.case
    );
    assert_eq!(
        member(CHECKSUM_TYPE),
        checksum_type,
        "case {:?}: expected a whole object's checksum type",
        leg.case
    );
    let e_tag = member(E_TAG);
    assert!(
        !e_tag.is_empty(),
        "case {:?}: the entry carries the object's ETag, got {e_tag:?}",
        leg.case
    );
    let last_modified = member(LAST_MODIFIED);
    assert!(
        last_modified.contains('T') && last_modified.ends_with('Z'),
        "case {:?}: expected an instant for LastModified, got {last_modified:?}",
        leg.case
    );

    // What the entry describes is the object itself: a read of that key answers the bytes the
    // buffer delivered, at the length the listing reported.
    let stored = emulator.object(&leg.bucket, &key).await;
    assert_eq!(
        String::from_utf8_lossy(&stored),
        String::from_utf8_lossy(&delivered),
        "case {:?}: the listed object holds the buffer's {} record(s) in arrival order ({} \
         bytes), got {} bytes",
        leg.case,
        leg.records.len(),
        delivered.len(),
        stored.len()
    );
}
