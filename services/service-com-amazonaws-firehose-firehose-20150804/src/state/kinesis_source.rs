// SPDX-License-Identifier: CC0-1.0

//! Consumption of the SOURCE a Firehose stream is fed by, where that source is a Kinesis
//! data stream.
//!
//! `PutRecord` / `PutRecordBatch` are how a `DirectPut` stream gets its data — "Provider
//! applications access the Firehose stream directly"
//! (`CreateDeliveryStreamInput$DeliveryStreamType`) — and a stream of any other type is fed
//! by the service its source configuration names instead, which is why the ingestion
//! operations are refused on one (`crate::state::DeliveryStream::takes_a_configured_source`).
//! This module is the other half of that rule for the one source whose data this emulator
//! holds: the Kinesis data stream of a `KinesisStreamAsSource` stream's
//! `KinesisStreamSourceConfiguration`, "the stream and role Amazon Resource Names (ARNs) for
//! a Kinesis data stream used as the source for a Firehose stream".
//!
//! What is consumed, and from where:
//!
//! * WHICH stream — the `KinesisStreamARN` of the stored `KinesisStreamSourceDescription`,
//!   resolved against the Amazon Kinesis Data Streams service's own store: the one register
//!   of streams, shards and records in this emulator, so what a producer's `PutRecord` wrote
//!   there is exactly what is read back here.
//! * UNDER WHOSE PERMISSIONS — the `RoleARN` that same source configuration names, which is
//!   the role the retrieval is made under, so it happens only where the permissions in force
//!   on that role allow the Kinesis Data Streams calls it is made of
//!   ([`source_read_authorization`]). A read the role may not make takes nothing, leaves the
//!   shard positions where they are, and is reported through the destination's own
//!   `CloudWatchLoggingOptions` — the only thing an operator can observe of it, since no
//!   client call is made for a source-fed stream at all.
//! * FROM WHERE — `DeliveryStartTimestamp`, since "Firehose starts retrieving records from
//!   the Kinesis data stream starting with this timestamp"
//!   (`KinesisStreamSourceDescription$DeliveryStartTimestamp`): a record the source held
//!   before the Firehose stream existed is not this stream's data. From then on each shard
//!   is read from the sequence number this stream last buffered from it
//!   ([`crate::state::DeliveryStream::source_positions`]), so every record is buffered
//!   exactly once and the shard's own order is kept.
//! * INTO WHAT — the stream's buffer, through the same append a direct put uses
//!   (`crate::state::push_record`), so from the buffer on a source-fed stream and a
//!   `DirectPut` stream are one path: the destination, the `BufferingHints` window and the
//!   error output of [`crate::state::delivery`] apply to both without knowing where the
//!   records came from.
//!
//! A record the buffer has no room for leaves its shard position where it was, so the next
//! poll takes it again rather than losing it — the buffer budget delays a record, it never
//! drops one.
//!
//! ## The clock
//!
//! Consumption is continuous on the real service and no further Firehose call is made for a
//! source-fed stream, so it is driven by the one in-process task
//! ([`crate::state::delivery::drive_delivery_clock`]), which polls each source every
//! [`SOURCE_POLL_SPAN`] and then delivers whatever that made due. Nothing here sleeps, and a
//! poll writes only what the source already held at the instant it is given.
//!
//! ## Lock order
//!
//! Three steps, each holding ONE store's lock: this service's store is read for the sources
//! it has, the Kinesis service's store is then read for their new records, and this service's
//! store is locked again to buffer them. The two locks are never held at once, and nothing in
//! the Kinesis service reaches into this one, so a cycle between them is unreachable. IAM is
//! asked between the first and the second step, with no lock of either store held.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_kinesis_kinesis_20131202::state::{
    sequence_number_le, ComAmazonawsKinesisKinesis20131202State,
};

use crate::state::delivery::destination_logging::DestinationLogging;
use crate::state::delivery::DeliveryHop;
use crate::state::{
    push_record, service_state, ComAmazonawsFirehoseFirehose20150804State, DeliveryStream,
    StreamStatus,
};

/// Whether the role a stream's source names may make the read at all, and what a read it may
/// not make reports.
pub mod source_read_authorization;

pub use source_read_authorization::{
    unauthorized_source_read, SourceReadDecision, KINESIS_ACCESS_DENIED, SOURCE_AUTHORIZATION_SPAN,
};

/// The `DeliveryStreamType` whose records come from a Kinesis data stream:
/// "`KinesisStreamAsSource`: The Firehose stream uses a Kinesis data stream as a source"
/// (`CreateDeliveryStreamInput$DeliveryStreamType`).
pub const KINESIS_STREAM_AS_SOURCE: &str = "KinesisStreamAsSource";

/// How long after one poll of a stream's source the next one comes due.
///
/// A CHOSEN span: AWS documents no polling interval for the source of a Firehose stream —
/// only that the records retrieved from it are buffered and then delivered — so this is the
/// emulator's, short enough that a producer writing to the source sees its data reach the
/// destination inside the delivery window it configured, and long enough that a stream with a
/// source costs one bounded read of the source register per span rather than a spin.
pub const SOURCE_POLL_SPAN: Duration = Duration::from_millis(200);

/// The most records ONE poll takes out of ONE shard of a source stream.
///
/// The documented ceiling of a single read of a shard — "Specify a value of up to 10,000"
/// (`GetRecordsInput$Limit` of the Kinesis Data Streams API, which is how the real service
/// reads its source) — so a poll of a shard holding more records than that takes the oldest
/// of them and the next poll continues from where it stopped.
const MAX_RECORDS_PER_SHARD_POLL: usize = 10_000;

/// The most record DATA one poll takes, over every shard of every source it reads.
///
/// The records a poll takes are held in this process before they are buffered, so what one
/// poll holds is bounded as well as what it counts: the documented ceiling of one read —
/// "The maximum size of data that GetRecords can return is 10 MiB"
/// (`service_com_amazonaws_kinesis_kinesis_20131202::state::MAX_GET_RECORDS_BYTES`) —
/// applied over the whole poll, so neither a source with many shards nor many streams with a
/// source can make one poll hold more than one read's worth. What a poll leaves behind keeps
/// its shard position, so the next poll takes it.
const MAX_BYTES_PER_POLL: usize =
    ::service_com_amazonaws_kinesis_kinesis_20131202::state::MAX_GET_RECORDS_BYTES;

/// Where one Firehose stream has consumed its source to, as a poll starts.
#[derive(Debug, Clone)]
pub(crate) struct SourceCursor {
    /// The Firehose stream's own name — the key its buffer is reached by.
    stream: String,
    /// The Firehose stream's own ARN, which is what an event reporting a read it could not
    /// make names, and whose Region that read's authorization question is asked in.
    stream_arn: String,
    /// `VersionId` as it stands at this poll, which such an event names too.
    version_id: u64,
    /// `KinesisStreamSourceDescription$KinesisStreamARN`: the source stream to read.
    source_arn: String,
    /// `KinesisStreamSourceDescription$RoleARN`: the role the retrieval is made under, which
    /// is what decides whether it may be made at all ([`source_read_authorization`]). `None`
    /// for a source configuration naming none.
    role_arn: Option<String>,
    /// Where each of this stream's destinations reports what went wrong, for the destinations
    /// whose `CloudWatchLoggingOptions` report anything at all.
    loggings: Vec<DestinationLogging>,
    /// `KinesisStreamSourceDescription$DeliveryStartTimestamp`, where the source carries one:
    /// the instant retrieval starts at, which is where a shard nothing has been consumed from
    /// yet is read from.
    delivery_start: Option<SystemTime>,
    /// The sequence number of the last record consumed from each shard, by shard id.
    positions: BTreeMap<String, String>,
    /// What IAM last answered about this stream's source read, with when it was asked
    /// ([`DeliveryStream::source_read_decision`]): the answer this poll stands on while it is
    /// still fresh.
    read_decision: Option<SourceReadDecision>,
}

/// One record a poll took out of a source stream, with where in the source it came from.
#[derive(Debug)]
struct SourceRecord {
    /// The shard the record was read from, whose position this record advances.
    shard_id: String,
    /// `Record$SequenceNumber` — "the unique identifier of the record within its shard".
    sequence_number: String,
    /// `Record$ApproximateArrivalTimestamp` — "the approximate time that the record was
    /// inserted into the stream", which is the order the records of several shards are
    /// buffered in.
    arrived_at: SystemTime,
    /// `Record$Data`, buffered byte for byte: "the data in the blob is both opaque and
    /// immutable to Kinesis Data Streams, which does not inspect, interpret, or change the
    /// data in the blob in any way".
    data: Vec<u8>,
}

/// Whether this stream's data comes from a Kinesis data stream it names, whatever its
/// lifecycle state is.
#[must_use]
pub fn has_kinesis_source(stream: &DeliveryStream) -> bool {
    stream.stream_type == KINESIS_STREAM_AS_SOURCE && source_arn(stream).is_some()
}

/// The `KinesisStreamARN` this stream's stored source description names, if it names one.
///
/// `pub(crate)` because the delivery of a stream's buffer names that source too: the
/// invocation of a destination's data transformation carries the Kinesis data stream feeding
/// the Firehose stream beside the Firehose stream itself
/// (`crate::state::delivery::lambda_processor`).
pub(crate) fn source_arn(stream: &DeliveryStream) -> Option<&str> {
    stream
        .source
        .as_ref()?
        .kinesis_stream_source_description
        .as_ref()?
        .kinesis_stream_arn
        .as_deref()
}

/// When the next poll of a source of this store comes due, or `None` when no stream of it has
/// one.
///
/// A span from `now` rather than a stored instant: what a poll finds is whatever the source
/// has taken since, so there is no earlier instant at which this store owes anything. This is
/// what keeps the delivery task awake for a stream nothing puts to — its data arrives at the
/// SOURCE, so no Firehose call is coming to start one.
#[must_use]
pub fn next_poll_at(
    store: &ComAmazonawsFirehoseFirehose20150804State,
    now: SystemTime,
) -> Option<SystemTime> {
    store
        .streams
        .values()
        .any(has_kinesis_source)
        .then(|| now + SOURCE_POLL_SPAN)
}

/// Consume what every source-fed stream of this store's sources holds at `now`, buffering it
/// for the delivery that follows.
///
/// Writes only records the source ALREADY held at `now`, so a caller reading the destination
/// sees nothing it would not have seen had this not been called: what is buffered here is
/// still delivered on its own `BufferingHints` window ([`crate::state::delivery`]).
///
/// `hop` is how the question each stream's retrieval has to put to IAM is asked — whether the
/// role its source configuration names may make the read at all
/// ([`source_read_authorization`]) — so a stream whose role may not reads nothing and reports
/// the refusal instead.
pub async fn ingest_from_sources<Eng: Send + Sync + 'static>(
    engine: &Eng,
    hop: &DeliveryHop,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // Step one: which streams have a source, and where each has read to. The lock is
    // released before the source register is read.
    let sources: Vec<SourceCursor> = {
        let store = state.lock();
        store.streams.values().filter_map(cursor_of).collect()
    };
    if sources.is_empty() {
        return;
    }
    // The retrieval is the SOURCE ROLE's to make, so what that role is allowed to do decides
    // whether it is made at all: a stream whose role IAM answers may not make the read reads
    // nothing this poll, and the refusal is reported through the stream's own logging as the
    // answer is taken. Asked with no store of this service locked.
    let mut readable = Vec::with_capacity(sources.len());
    for cursor in sources {
        if source_read_authorization::source_read_refusal(engine, hop, &cursor, now)
            .await
            .is_none()
        {
            readable.push(cursor);
        }
    }
    if readable.is_empty() {
        return;
    }
    let Some(kinesis) =
        ::service_com_amazonaws_kinesis_kinesis_20131202::state::service_state(engine)
    else {
        ::tracing::warn!(
            "the Kinesis data stream store is not reachable from this engine, so no Firehose \
             stream's source is consumed"
        );
        return;
    };
    // Step two: the records each source has taken since this stream last read it. ONE byte
    // budget over the whole poll, since every record it takes is held in this process until
    // step three buffers it.
    let fetched: Vec<(String, Vec<SourceRecord>)> = {
        let source_store = kinesis.lock();
        let mut budget = MAX_BYTES_PER_POLL;
        readable
            .into_iter()
            .map(|cursor| {
                let records = records_after(&source_store, &cursor, now, &mut budget);
                (cursor.stream, records)
            })
            .collect()
    };
    // Step three: buffer them, which is where a stream's shard positions advance.
    let mut store = state.lock();
    for (stream, records) in fetched {
        buffer_records(&mut store, &stream, records, now);
    }
}

/// Where `stream` has consumed its source to, or `None` when it has no source to consume.
///
/// A stream that is not `ACTIVE` consumes nothing: a created stream starts in `CREATING` and
/// is not serving its data plane yet, and a deleted one is on its way out. Nothing is lost by
/// waiting — the shard positions are untouched, so the first poll after the stream is active
/// still reads from its `DeliveryStartTimestamp`.
fn cursor_of(stream: &DeliveryStream) -> Option<SourceCursor> {
    if stream.status != StreamStatus::Active {
        return None;
    }
    let source = stream
        .source
        .as_ref()?
        .kinesis_stream_source_description
        .as_ref()?;
    let source_arn = source.kinesis_stream_arn.clone()?;
    if stream.stream_type != KINESIS_STREAM_AS_SOURCE {
        return None;
    }
    Some(SourceCursor {
        stream: stream.name.clone(),
        stream_arn: stream.arn.clone(),
        version_id: stream.version_id,
        source_arn,
        role_arn: source.role_arn.clone(),
        loggings: stream
            .destinations
            .iter()
            .filter_map(|destination| {
                DestinationLogging::of(destination.config.cloud_watch_logging_options())
            })
            .collect(),
        delivery_start: source
            .delivery_start_timestamp
            .and_then(|at| SystemTime::try_from(at).ok()),
        positions: stream.source_positions.clone(),
        read_decision: stream.source_read_decision,
    })
}

/// The records of `cursor`'s source stream that it has not consumed yet, oldest first.
///
/// Empty for a source this emulator does not hold, or one that is not serving its shards
/// (still being created, or already deleted): there is nothing to read yet, and the positions
/// stay where they are, so a stream created before its source is serving starts consuming as
/// soon as it does.
fn records_after(
    source_store: &ComAmazonawsKinesisKinesis20131202State,
    cursor: &SourceCursor,
    now: SystemTime,
    budget: &mut usize,
) -> Vec<SourceRecord> {
    let Some(region) = arn_region(&cursor.source_arn) else {
        return Vec::new();
    };
    let Some(source) = source_store.stream_by_arn(now, region, &cursor.source_arn) else {
        return Vec::new();
    };
    if !source.serves_shards(now) {
        return Vec::new();
    }
    // A record the source has already trimmed is no longer readable there — "a stream's data
    // records are accessible for 24 hours from the time they are added to the stream" and for
    // its `RetentionPeriodHours` once that window is changed — so the horizon bounds what a
    // consumer of it can take, exactly as it bounds `GetRecords`.
    let trim_horizon = source.trim_horizon(now);
    let mut taken = Vec::new();
    for shard in &source.shards {
        let mut from_this_shard = 0usize;
        for record in unconsumed(shard, cursor) {
            if from_this_shard >= MAX_RECORDS_PER_SHARD_POLL || *budget == 0 {
                break;
            }
            // A record the source no longer retains is not read, whatever the position says.
            if record.arrived_at < trim_horizon {
                continue;
            }
            *budget = budget.saturating_sub(record.data.len());
            taken.push(SourceRecord {
                shard_id: shard.shard_id.clone(),
                sequence_number: record.sequence_number.clone(),
                arrived_at: record.arrived_at,
                data: record.data.clone(),
            });
            from_this_shard += 1;
        }
    }
    // One buffer takes the records of every shard of the source, so they are ordered by when
    // the source took them: within one shard that is the shard's own order (a sequence number
    // grows with every write to it), and across shards it is the order the producers wrote
    // them in.
    taken.sort_by(|left, right| {
        (left.arrived_at, &left.shard_id)
            .cmp(&(right.arrived_at, &right.shard_id))
            .then_with(|| sequence_order(&left.sequence_number, &right.sequence_number))
    });
    taken
}

/// The records of `shard` this stream has not consumed yet, in the shard's own order.
///
/// Both of the two starting points are a PREFIX of the shard: a shard appends in arrival
/// order and mints a sequence number that grows with every write, so the records at or before
/// the consumed position are a prefix and so are the records that arrived before retrieval
/// started. The prefix is skipped rather than scanned, so one poll costs what the shard has
/// taken since the last one and not what the shard holds.
fn unconsumed<'shard>(
    shard: &'shard ::service_com_amazonaws_kinesis_kinesis_20131202::state::ShardRecord,
    cursor: &SourceCursor,
) -> &'shard [::service_com_amazonaws_kinesis_kinesis_20131202::state::DataRecord] {
    let consumed = match cursor.positions.get(&shard.shard_id) {
        Some(last) => shard
            .records
            .partition_point(|record| sequence_number_le(&record.sequence_number, last)),
        // Nothing taken from the shard yet, so retrieval starts at the delivery start
        // timestamp: "Firehose starts retrieving records from the Kinesis data stream starting
        // with this timestamp."
        None => match cursor.delivery_start {
            Some(from) => shard
                .records
                .partition_point(|record| record.arrived_at < from),
            None => 0,
        },
    };
    shard.records.get(consumed..).unwrap_or_default()
}

/// The order two sequence numbers are in, read through the source register's OWN comparison
/// ([`sequence_number_le`]) rather than through a second spelling of it: `SequenceNumber` is
/// a decimal integer wider than any integer type this crate holds, so that function is what
/// says what its numeric order is.
fn sequence_order(left: &str, right: &str) -> ::std::cmp::Ordering {
    match (
        sequence_number_le(left, right),
        sequence_number_le(right, left),
    ) {
        (true, true) => ::std::cmp::Ordering::Equal,
        (true, false) => ::std::cmp::Ordering::Less,
        (false, _) => ::std::cmp::Ordering::Greater,
    }
}

/// The region of a Kinesis stream ARN — `arn:<partition>:kinesis:<region>:<account>:stream/<name>`,
/// the form `KinesisStreamARN`'s own modelled pattern requires — or `None` for a value that
/// carries none.
fn arn_region(arn: &str) -> Option<&str> {
    let region = arn.split(':').nth(3)?;
    (!region.is_empty()).then_some(region)
}

/// Append `records` to the buffer of the stream `name`, advancing its shard positions as each
/// one is stored.
///
/// A record the buffer has no room for stops the poll with its shard position unchanged, so
/// that record and everything after it is taken again by a later poll: the budget delays a
/// record rather than dropping one, exactly as a refused put leaves nothing stored.
fn buffer_records(
    store: &mut ComAmazonawsFirehoseFirehose20150804State,
    name: &str,
    records: Vec<SourceRecord>,
    now: SystemTime,
) {
    if records.is_empty() {
        return;
    }
    let counter = &mut store.record_counter;
    let buffered_bytes = &mut store.buffered_bytes;
    let Some(stream) = store.streams.get_mut(name) else {
        return;
    };
    // The stream may have been deleted, or its activation reverted, between the read of its
    // source and this append: a stream that is not accepting data buffers nothing.
    if stream.status != StreamStatus::Active {
        return;
    }
    for record in records {
        if push_record(stream, counter, buffered_bytes, record.data, now).is_none() {
            break;
        }
        stream
            .source_positions
            .insert(record.shard_id, record.sequence_number);
    }
}
