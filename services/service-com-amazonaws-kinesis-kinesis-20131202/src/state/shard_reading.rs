// SPDX-License-Identifier: CC0-1.0

//! Reading ONE shard from the position a request names: where that position is, and what one
//! read of the shard from it delivers.
//!
//! Two operations of this service serve a shard's records to a reader — `GetRecords`, which
//! reads a page at the position its shard iterator carries, and `SubscribeToShard`, which
//! pushes the same records to a registered consumer over an event stream — and the model
//! gives them the same members to name a position with: a `ShardIteratorType` naming it, plus
//! the sequence number or the instant the three types that take one document
//! (`GetShardIteratorInput$ShardIteratorType`, `StartingPosition$Type`). The rule is ONE
//! rule, so it lives here once rather than in each operation:
//!
//! * [`requested_shard_position`] resolves the members a request carried into a
//!   [`ShardPosition`], or states why they name no position — the sentence alone, so each
//!   operation answers it with the error ITS model declares and names the members ITS
//!   request spells ([`PositionMembers`]);
//! * [`read_shard_from`] is one bounded read of the shard from such a position: the records
//!   it delivers, how far the reader has got, and how far behind the tip of the shard it now
//!   is.
//!
//! Neither writes anything: every part of a position travels in the request (inside a shard
//! iterator, or as the `StartingPosition` member), so reading a shard leaves it exactly as a
//! later reader finds it.

use ::std::time::SystemTime;

use super::{
    epoch_millis, sequence_number_le, DataRecord, ShardPosition, ShardRecord,
    SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER, SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER,
    SHARD_ITERATOR_TYPE_AT_TIMESTAMP, SHARD_ITERATOR_TYPE_LATEST,
    SHARD_ITERATOR_TYPE_TRIM_HORIZON, TIMESTAMP_NOT_AN_EPOCH_DATE_MESSAGE,
};
use crate::ops::types::SHARD_ITERATOR_TYPE_VALUES;

/// The members ONE operation's request spells a starting position with, as its refusals name
/// them.
///
/// The rule below is the same for both readers; what differs is the member a caller set, and
/// a refusal states the position at fault the way the request spelled it — `GetRecords`
/// reaches the position through `GetShardIterator`'s own members, `SubscribeToShard` carries
/// them inside its `StartingPosition` structure.
#[derive(Debug, Clone, Copy)]
pub struct PositionMembers {
    /// The member naming which of the `ShardIteratorType` values the position is.
    pub position_type: &'static str,
    /// The member supplying the sequence number the two sequence-number types read.
    pub sequence_number: &'static str,
    /// The member supplying the instant `AT_TIMESTAMP` reads.
    pub timestamp: &'static str,
}

/// How a reader states that the `ShardIteratorType` it was given and the sequence number it
/// was (or was not) given do not go together: the enumeration of the legal combinations, then
/// the combination the request actually named.
///
/// The rule is the one each type documents — "AT_SEQUENCE_NUMBER - Start reading from the
/// position denoted by a specific sequence number, provided in the value
/// StartingSequenceNumber", the same for AFTER_SEQUENCE_NUMBER, against "TRIM_HORIZON - Start
/// reading at the last untrimmed record" and "LATEST - Start reading just after the most
/// recent record", which read no sequence number at all
/// (`GetShardIteratorInput$ShardIteratorType`) — so the sentence states BOTH halves of it and
/// then what it was handed instead. Everything but the type the request named, whether it
/// carried a sequence number, and the member it spells one with ([`PositionMembers`]) is fixed
/// text: one sentence for the one rule, whichever reader states it.
fn inconsistent_sequence_number_message(
    members: PositionMembers,
    position_type: &str,
    carried_sequence_number: bool,
) -> String {
    let member = members.sequence_number;
    format!(
        "Must either specify (1) {SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER} or \
         {SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER} and {member} or (2) \
         {SHARD_ITERATOR_TYPE_TRIM_HORIZON} or {SHARD_ITERATOR_TYPE_LATEST} and no {member}. \
         Request specified {position_type} and {}{member}.",
        if carried_sequence_number { "" } else { "no " }
    )
}

/// Where in `shard` a reader that named `position_type` starts, refusing a request whose
/// members that type does not admit.
///
/// Each type documents which member supplies its position — "AT_SEQUENCE_NUMBER - Start
/// reading from the position denoted by a specific sequence number", the same for
/// AFTER_SEQUENCE_NUMBER, and "AT_TIMESTAMP - Start reading from the position denoted by a
/// specific time stamp" — so a request that omits that member names no position, and one
/// that carries a member its type does not read is refused rather than answered with a
/// position it did not ask for.
///
/// `LATEST` is PINNED here rather than resolved when the records are read: "LATEST - Start
/// reading just after the most recent record in the shard" names the most recent record at
/// the instant the position is resolved, so a record written afterwards is read by this
/// reader rather than skipped by it.
///
/// `Err` carries the refusal SENTENCE: the modelled error that states it is the calling
/// operation's ("A specified parameter exceeds its restrictions, is not supported, or can't
/// be used", `InvalidArgumentException`, for both of this service's readers).
pub fn requested_shard_position(
    members: PositionMembers,
    position_type: &str,
    sequence_number: Option<&str>,
    timestamp: Option<::aws_smithy_types::DateTime>,
    shard: &ShardRecord,
    trim_horizon: SystemTime,
) -> Result<ShardPosition, String> {
    let trim_horizon_millis = epoch_millis(trim_horizon);
    // `ShardIteratorType` is a modelled enum, so the request walk refuses an unknown value
    // before a handler runs; this states the same refusal for a caller that reaches one
    // another way.
    if !SHARD_ITERATOR_TYPE_VALUES.contains(&position_type) {
        return Err(format!(
            "{} {position_type} is not supported; the supported values are {}",
            members.position_type,
            SHARD_ITERATOR_TYPE_VALUES.join(" | ")
        ));
    }
    let takes_sequence_number = position_type == SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER
        || position_type == SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER;
    let takes_timestamp = position_type == SHARD_ITERATOR_TYPE_AT_TIMESTAMP;
    if sequence_number.is_some() && !takes_sequence_number {
        return Err(inconsistent_sequence_number_message(
            members,
            position_type,
            true,
        ));
    }
    if timestamp.is_some() && !takes_timestamp {
        return Err(format!(
            "{} can only be used with the {SHARD_ITERATOR_TYPE_AT_TIMESTAMP} shard iterator \
             type, not with {position_type}",
            members.timestamp
        ));
    }
    if takes_sequence_number {
        let Some(sequence_number) = sequence_number else {
            return Err(inconsistent_sequence_number_message(
                members,
                position_type,
                false,
            ));
        };
        // A sequence number is "the identifier associated with every record ingested in the
        // stream", and every one a shard can hold lies inside the range the shard publishes
        // (`Shard$SequenceNumberRange`): a value outside it names a position in no shard.
        let inside = sequence_number_le(&shard.starting_sequence_number, sequence_number)
            && shard
                .ending_sequence_number
                .as_deref()
                .is_none_or(|ending| sequence_number_le(sequence_number, ending));
        if !inside {
            return Err(format!(
                "{} {sequence_number} is not a sequence number of shard {}",
                members.sequence_number, shard.shard_id
            ));
        }
        return Ok(if position_type == SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER {
            ShardPosition::AtSequenceNumber(sequence_number.to_string())
        } else {
            ShardPosition::AfterSequenceNumber(sequence_number.to_string())
        });
    }
    if takes_timestamp {
        let Some(at) = timestamp else {
            return Err(format!(
                "{} is required with the {position_type} shard iterator type",
                members.timestamp
            ));
        };
        let Ok(millis) = at.to_millis() else {
            return Err(TIMESTAMP_NOT_AN_EPOCH_DATE_MESSAGE.to_string());
        };
        let millis = u128::try_from(millis).unwrap_or_default();
        // "If the time stamp is older than the current trim horizon, the iterator returned
        // is for the oldest untrimmed data record (TRIM_HORIZON)."
        return Ok(if millis < trim_horizon_millis {
            ShardPosition::TrimHorizon
        } else {
            ShardPosition::AtTimestamp(millis)
        });
    }
    if position_type == SHARD_ITERATOR_TYPE_TRIM_HORIZON {
        return Ok(ShardPosition::TrimHorizon);
    }
    // The most recent record the shard holds right now is what `LATEST` starts just after.
    // A shard that holds none is pinned by the position itself: every record it holds when
    // it is read arrived after this position was resolved, so all of them are read.
    Ok(match shard.retained(trim_horizon).next_back() {
        Some(newest) => ShardPosition::AfterSequenceNumber(newest.sequence_number.clone()),
        None => ShardPosition::Latest,
    })
}

/// Where in the retained records of a shard `position` starts, as an index into `retained`;
/// the index one past the newest record when the position names nothing the shard holds yet,
/// which is an empty read rather than a refusal ("If there are no records available in the
/// portion of the shard that the iterator points to, GetRecords returns an empty list").
fn start_index(position: &ShardPosition, retained: &[&DataRecord]) -> usize {
    let after_the_newest = retained.len();
    match position {
        // "the last untrimmed record in the shard in the system, which is the oldest data
        // record in the shard" — the first record still inside the retention window.
        ShardPosition::TrimHorizon => 0,
        // "just after the most recent record in the shard", pinned when the position was
        // resolved: a reader carries this position only for a shard that held NO record then
        // (a shard that held one is pinned to it by sequence number), so every record the
        // shard holds now arrived after that instant and all of them are read.
        ShardPosition::Latest => 0,
        ShardPosition::AtSequenceNumber(sequence_number) => retained
            .iter()
            .position(|record| sequence_number_le(sequence_number, &record.sequence_number))
            .unwrap_or(after_the_newest),
        ShardPosition::AfterSequenceNumber(sequence_number) => retained
            .iter()
            .position(|record| !sequence_number_le(&record.sequence_number, sequence_number))
            .unwrap_or(after_the_newest),
        ShardPosition::AtTimestamp(millis) => retained
            .iter()
            .position(|record| epoch_millis(record.arrived_at) >= *millis)
            .unwrap_or(after_the_newest),
    }
}

/// ONE bounded read of a shard from a position: what the reader is served, and where it now
/// stands in the shard.
#[derive(Debug)]
pub struct ShardRead<'store> {
    /// The records the read delivers, oldest first.
    pub records: Vec<&'store DataRecord>,
    /// The newest record the reader has now seen — the last one delivered, or, for a
    /// position that skipped past older records and delivered none, the last one it skipped.
    /// `None` when the reader has seen nothing the shard retains.
    pub last_seen: Option<&'store DataRecord>,
    /// Whether the reader has now seen every record the shard retains.
    pub drained: bool,
    /// "The number of milliseconds the response is from the tip of the stream, indicating
    /// how far behind current time the consumer is. A value of zero indicates that record
    /// processing is caught up, and there are no new records to process at this moment"
    /// (`GetRecordsOutput$MillisBehindLatest`, and the same member of
    /// `SubscribeToShardEvent`).
    pub millis_behind_latest: i64,
}

/// Read `retained` from `position`, delivering at most `max_records` records and at most
/// `max_bytes` of record DATA — but never stopping short of the read's first record, which a
/// reader would otherwise be unable to get past at all.
///
/// The byte ceiling is measured over the DATA the read answers with, which is what the
/// published per-read size bounds ("The maximum size of data that GetRecords can return is 10
/// MiB"); a record's partition key counts towards what the store holds and what a write may
/// carry, not towards what a read returns.
pub fn read_shard_from<'store>(
    position: &ShardPosition,
    retained: &[&'store DataRecord],
    max_records: usize,
    max_bytes: usize,
    now: SystemTime,
) -> ShardRead<'store> {
    let start = start_index(position, retained);
    let mut records: Vec<&'store DataRecord> = Vec::new();
    let mut read_bytes = 0usize;
    for record in retained.iter().skip(start) {
        if records.len() >= max_records {
            break;
        }
        let with_record = read_bytes.saturating_add(record.data.len());
        if !records.is_empty() && with_record > max_bytes {
            break;
        }
        read_bytes = with_record;
        records.push(record);
    }
    // Everything up to here is what the reader has now seen: the records it was handed and,
    // for a position that skipped past older ones, those too.
    let seen = start.saturating_add(records.len());
    let drained = seen == retained.len();
    let last_seen = seen
        .checked_sub(1)
        .and_then(|index| retained.get(index))
        .copied();

    // A reader that has drained the shard is caught up, so its lag is zero however long ago
    // the last record arrived; a reader with records still ahead of it is behind by how long
    // ago the newest record it HAS seen arrived — the age of the newest data it holds,
    // measured against the clock rather than against the tip's own instant, which is what
    // "behind current time" names. (Measuring from the first record it has NOT seen would
    // answer almost the same number and would report a lag for a reader that is caught up
    // with everything retained.)
    let millis_behind_latest = if drained {
        0
    } else {
        let behind_from = last_seen
            .or_else(|| retained.first().copied())
            .map(|record| epoch_millis(now).saturating_sub(epoch_millis(record.arrived_at)))
            .unwrap_or_default();
        i64::try_from(behind_from).unwrap_or(i64::MAX)
    };

    ShardRead {
        records,
        last_seen,
        drained,
        millis_behind_latest,
    }
}
