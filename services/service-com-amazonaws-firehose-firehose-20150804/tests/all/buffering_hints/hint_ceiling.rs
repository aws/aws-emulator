// SPDX-License-Identifier: CC0-1.0

//! What a destination asking for an interval at the CEILING of the published range gets: the
//! window it asked for is not collapsed into its own put.
//!
//! The rule under test: `BufferingHints$IntervalInSeconds` is how long a destination's records
//! are held before they are written — "buffer incoming data for the specified period of time,
//! in seconds, before delivering it to the destination" — so the interval a destination
//! configures is the wait its records get while that interval is inside the span this emulator
//! holds data for, and only the OTHER buffering condition can flush a buffer earlier
//! ("Firehose buffers data up to 5 MB or for 5 minutes, whichever condition is satisfied
//! first" — `CreateDeliveryStream`). A destination asking for minutes therefore still holds a
//! handful of small records a long time after the put, while one asking for a window of
//! seconds has them written at the end of that window: nothing is delivered before the
//! destination's own hints are met. How a configuration is SPELLED makes no difference to that
//! — the documented default interval stated explicitly is the same configuration as the member
//! left out — and neither does a tag mutation on the stream, because `TagDeliveryStream` /
//! `UntagDeliveryStream` carry no delivery semantics.
//!
//! The parent suite [`crate::buffering_hints`] pins the same rule on the windows a case can
//! wait out — the published minimum, a short non-zero one, and the size condition firing
//! inside a long interval. What this child adds is the TOP of the published range: a
//! destination whose interval is minutes away holds its records all through a window that the
//! rows beside it show is far longer than the span a prompt delivery takes, so a window of
//! minutes is not quietly collapsed into the put that opened the buffer. Because the hints are
//! hints the service may improve on, an interval longer than the span this emulator holds data
//! for is flushed at the end of that span rather than waited out; that END of the window is the
//! sibling suite [`super::delivered_listing_entry`], and every row here is read well inside the
//! span, where it must hold nothing.
//!
//! No case here sleeps a window out or reads the wall clock. The control row, configured with
//! the shortest interval its member admits, is polled until its object is there; the row whose
//! interval is tens of seconds is polled until its own object is there, which is what says the
//! wait passed; and the rows whose interval is minutes away are required to hold NOTHING at
//! every reading taken while both of those are waited for.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::Emulator;
use crate::parquet_serializer_parameters::await_active;

/// The bucket every row here delivers into; each row has its own `Prefix` under it, so a
/// listing is per case.
const BUCKET: &str = "hint-ceiling-bucket";

/// The interval of the row whose window a case waits out, in seconds.
///
/// Inside the published range of `IntervalInSeconds` (0..900), and tens of seconds: longer
/// than the span any delivery this emulator makes without being asked would take, so the
/// readings taken while it is waited for are what the rows asking for MINUTES must hold
/// nothing at.
const TENS_OF_SECONDS: i64 = 25;

/// How long the row whose interval is [`TENS_OF_SECONDS`] is waited for before the case fails.
///
/// Generous beside that interval, so a loaded machine does not fail a case the clock would
/// have served, and inside the span this emulator holds data for
/// (`state::delivery::MAX_BUFFER_HOLD`), so the rows asking for MINUTES are still holding
/// their records at every reading taken while this one is waited for; it bounds nothing the
/// case measures.
const WAITED_OUT_DELIVERY_DEADLINE: Duration = Duration::from_secs(45);

/// How long the control row's delivery — the one asking for the shortest interval its member
/// admits — is waited for before the case fails.
const PROMPT_DELIVERY_DEADLINE: Duration = Duration::from_secs(30);

/// How often a destination is polled while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// When a row's records may first appear at its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Due {
    /// At once: the row asks for the shortest interval `IntervalInSeconds` admits.
    AtOnce,
    /// Once the window it asked for has passed, which this suite waits out.
    AfterItsInterval,
    /// Not while this case runs: its interval is minutes away — past the span this emulator
    /// holds data for, and far past every reading this case takes — and its records are far
    /// below its size.
    NotYet,
}

/// What the stream's tags are made to do between the create and the put.
///
/// `pub(super)` so the sibling suite reading what the destination's listing says about the
/// delivered object states its rows' tag mutations in the same terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tagging {
    /// Nothing: the stream is never tagged.
    Untouched,
    /// Tagged, re-tagged with another value, and then stripped of its last tag — the
    /// mutations `TagDeliveryStream` and `UntagDeliveryStream` perform, neither of which says
    /// anything about delivery.
    ChangedAndRemoved,
}

/// One case: how the destination's `BufferingHints` are spelled, what happens to the stream's
/// tags, and when its records are due.
struct Row {
    case: &'static str,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`; `None` leaves the member out
    /// altogether, which is the documented defaults.
    hints: Option<(i64, i64)>,
    /// How many records are put, one `PutRecord` each.
    records: usize,
    tagging: Tagging,
    due: Due,
}

/// The cases.
///
/// The rows differ along what the rule generalises over: the configured `IntervalInSeconds`
/// (the published minimum 0 as the control, a window of tens of seconds, the documented
/// default 300 stated explicitly, the member left out so that default stands unstated, and
/// the published maximum 900), the configured `SizeInMBs` beside it (the documented default 5,
/// the published minimum 1 and the published maximum 128 — none of which a handful of small
/// records comes near), how many records one buffer holds, and whether the stream's tags are
/// changed and removed between the create and the put. The published ranges are `SizeInMBs`
/// 1..128 and `IntervalInSeconds` 0..900 (`smithy_query(query="shape",
/// shape="SizeInMBs,IntervalInSeconds")`), and the defaults are `BufferingHints`' own ("The
/// default value is 5", "The default value is 300").
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the documented defaults stated explicitly, with the tags changed and \
                   then removed",
            hints: Some((5, 300)),
            records: 1,
            tagging: Tagging::ChangedAndRemoved,
            due: Due::NotYet,
        },
        Row {
            case: "the same defaults stated explicitly, with the stream never tagged",
            hints: Some((5, 300)),
            records: 1,
            tagging: Tagging::Untouched,
            due: Due::NotYet,
        },
        Row {
            case: "no BufferingHints member at all, so the documented default interval \
                   stands unstated",
            hints: None,
            records: 2,
            tagging: Tagging::Untouched,
            due: Due::NotYet,
        },
        Row {
            case: "the longest interval the member admits, under the largest size it admits",
            hints: Some((128, 900)),
            records: 3,
            tagging: Tagging::ChangedAndRemoved,
            due: Due::NotYet,
        },
        Row {
            case: "an interval of tens of seconds, which this case waits out",
            hints: Some((1, TENS_OF_SECONDS)),
            records: 2,
            tagging: Tagging::Untouched,
            due: Due::AfterItsInterval,
        },
        Row {
            case: "the shortest interval the member admits, as the control",
            hints: Some((1, 0)),
            records: 1,
            tagging: Tagging::ChangedAndRemoved,
            due: Due::AtOnce,
        },
    ]
}

/// One row under test: its stream, where it delivers and what its producer put.
struct Leg {
    case: &'static str,
    stream: String,
    /// The `Prefix` its destination is configured with, which isolates its objects.
    prefix: String,
    /// The records put, in arrival order.
    records: Vec<Vec<u8>>,
    tagging: Tagging,
    due: Due,
}

#[tokio::test]
async fn an_interval_of_minutes_is_not_collapsed_into_the_put_that_opened_the_buffer() {
    let emulator = Emulator::new();
    emulator.create_bucket(BUCKET).await;

    // Every stream is created before any of them is awaited, so the suite spends ONE
    // clock-owed CREATING -> ACTIVE span rather than one per row.
    let mut legs: Vec<Leg> = Vec::new();
    for (ordinal, row) in rows().into_iter().enumerate() {
        let stream = format!("hint-ceiling-stream-{ordinal}");
        let prefix = format!("row-{ordinal}/");
        emulator
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": &stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration":
                        destination(BUCKET, &prefix, row.hints),
                }),
            )
            .await;
        legs.push(Leg {
            case: row.case,
            stream,
            prefix,
            records: (0..row.records)
                .map(|index| format!("row-{ordinal}-record-{index}").into_bytes())
                .collect(),
            tagging: row.tagging,
            due: row.due,
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
    // emulator's own delivery clock does with the records these accepted.
    for leg in &legs {
        for record in &leg.records {
            let (status, answered) = emulator.put_record(&leg.stream, record).await;
            assert_eq!(
                status, 200,
                "case {:?}: the put must be accepted, got {answered:?}",
                leg.case
            );
        }
    }

    // The control row is delivered with no window to wait out; at the reading that says so,
    // every row with a window still running holds nothing, so its records are being HELD
    // rather than written by the put.
    for leg in legs.iter().filter(|leg| leg.due == Due::AtOnce) {
        await_object(&emulator, leg, PROMPT_DELIVERY_DEADLINE, &legs).await;
    }
    assert_nothing_delivered(&emulator, &legs).await;

    // The row whose window is tens of seconds is delivered once that window has passed — and
    // all through the wait for it, every row whose interval is MINUTES away still holds
    // nothing: one row's turn of the cadence does not write another row's buffer.
    for leg in legs.iter().filter(|leg| leg.due == Due::AfterItsInterval) {
        await_object(&emulator, leg, WAITED_OUT_DELIVERY_DEADLINE, &legs).await;
    }
    assert_nothing_delivered(&emulator, &legs).await;
}

/// The `ExtendedS3DestinationConfiguration` a row is created with: a location in `bucket`
/// under `prefix`, carrying `hints` when the row states them.
fn destination(bucket: &str, prefix: &str, hints: Option<(i64, i64)>) -> Value {
    let mut config = json!({
        "RoleARN": ROLE_ARN,
        "BucketARN": format!("arn:aws:s3:::{bucket}"),
        "Prefix": prefix,
        "CompressionFormat": "UNCOMPRESSED",
    });
    if let Some((size, interval)) = hints {
        config["BufferingHints"] = json!({
            "SizeInMBs": size,
            "IntervalInSeconds": interval,
        });
    }
    config
}

/// Tag `stream`, change that tag's value, and then remove the last tag it carries — a tag
/// mutation that says nothing about delivery.
///
/// `pub(super)` so the sibling suite reading what the destination's listing says about the
/// delivered object mutates its streams' tags the same way rather than re-spelling the calls.
pub(super) async fn change_and_remove_tags(emulator: &Emulator, case: &str, stream: &str) {
    for value in ["first", "second"] {
        emulator
            .firehose_ok(
                "TagDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "Tags": [{ "Key": "stage", "Value": value }],
                }),
            )
            .await;
    }
    emulator
        .firehose_ok(
            "UntagDeliveryStream",
            json!({ "DeliveryStreamName": stream, "TagKeys": ["stage"] }),
        )
        .await;
    let listed = emulator
        .firehose_ok(
            "ListTagsForDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    let tags = listed["Tags"].as_array().cloned().unwrap_or_default();
    assert!(
        tags.is_empty(),
        "case {case:?}: the last tag was removed, so none is left, got {tags:?}"
    );
}

/// Require every leg whose interval is minutes away to hold nothing: its buffer's own
/// conditions are not met at any reading this case takes, so its records are still held.
async fn assert_nothing_delivered(emulator: &Emulator, legs: &[Leg]) {
    for leg in legs.iter().filter(|leg| leg.due == Due::NotYet) {
        let keys = emulator.keys_under(BUCKET, &leg.prefix).await;
        assert!(
            keys.is_empty(),
            "case {:?}: a buffer whose interval is still running holds its records, \
             got {keys:?}",
            leg.case
        );
    }
}

/// Wait until `leg`'s destination holds exactly one object carrying every record of its buffer
/// in arrival order, or fail naming the case once `deadline` has passed. Every leg whose
/// interval is minutes away is read at each turn of the wait and must hold nothing.
async fn await_object(emulator: &Emulator, leg: &Leg, deadline: Duration, legs: &[Leg]) {
    let expire = Instant::now() + deadline;
    let keys = loop {
        let keys = emulator.keys_under(BUCKET, &leg.prefix).await;
        if !keys.is_empty() {
            break keys;
        }
        assert_nothing_delivered(emulator, legs).await;
        assert!(
            Instant::now() < expire,
            "case {:?}: expected the records put to be delivered under {:?}, it holds nothing",
            leg.case,
            leg.prefix
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        keys.len(),
        1,
        "case {:?}: one flushed buffer is one object, got {keys:?}",
        leg.case
    );
    let body = emulator.object(BUCKET, &keys[0]).await;
    let expected = leg.records.concat();
    assert!(
        body == expected,
        "case {:?}: the object holds the buffer's {} record(s) in arrival order ({} bytes), \
         got {} bytes: {:?}",
        leg.case,
        leg.records.len(),
        expected.len(),
        body.len(),
        String::from_utf8_lossy(&body)
    );
}
