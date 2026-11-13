// SPDX-License-Identifier: CC0-1.0

//! The WRITE quota one shard admits records at, and how a write past it is stated.
//!
//! "Each shard can support writes up to 1,000 records per second, up to a maximum data write
//! total of 1 MB per second" (`PutRecords`) — a rate PER SHARD, so it is accounted on the
//! shard a record is placed in rather than on the stream or on the call. A record the shard
//! cannot take is not stored: `PutRecords` reports it inside its 200 as a failed entry
//! ("ErrorCodes can be either ProvisionedThroughputExceededException or InternalFailure",
//! `PutRecordsResultEntry$ErrorCode`) while its accepted siblings are stored, and `PutRecord`
//! — one record, nothing to report per entry — is refused with the operation's own
//! `ProvisionedThroughputExceededException`.
//!
//! The accounting is the tree's one token bucket ([`::service_common::token_bucket`]): the
//! published rate, refilled from the time since the last admitted write, so a shard nobody
//! has written to takes its whole capacity at once and one written to back to back takes what
//! the rate refilled. Nothing here sleeps or polls: the capacity is computed at the instant a
//! write asks.
//!
//! The CAPACITY is a second of the rate plus the largest record the stream admits
//! ([`byte_limits`]), because the two published numbers have to hold together: a stream
//! configured for large records ("Amazon Kinesis Data Streams supports large records up to 10
//! MiB in size", `GetRecords`) must be able to write one, which a capacity of a second's
//! write bytes alone would refuse outright, and one record may be up to ten times that
//! second. The steady-state rate is still the published one, which is what bounds a producer
//! that keeps writing.

use ::std::time::Instant;

use ::service_common::token_bucket::{Bucket, Limits};

/// The records one shard admits per second: "Each shard can support writes up to 1,000
/// records per second" (`PutRecords`).
pub const SHARD_WRITE_RECORDS_PER_SECOND: f64 = 1_000.0;

/// The bytes one shard admits per second: "up to a maximum data write total of 1 MB per
/// second" (`PutRecords`), in the mebibyte a shard's write throughput is published in ("1
/// MiB/sec of write throughput", the ingest half of `WarmThroughputMiBps`).
pub const SHARD_WRITE_BYTES_PER_SECOND: f64 = 1024.0 * 1024.0;

/// The record-count bucket's rate and burst: the published rate, with one second of it as
/// the capacity.
const RECORD_LIMITS: Limits = Limits {
    rate: SHARD_WRITE_RECORDS_PER_SECOND,
    burst: SHARD_WRITE_RECORDS_PER_SECOND,
};

/// The largest record ANY stream may be configured for, in bytes: `MaxRecordSizeInKiB` is
/// modelled `range 1024..10240`, so ten mebibytes. What a shard holds before anything is
/// written to it ([`WriteQuota::fresh`]), so a fresh shard holds its whole capacity whatever
/// its stream is configured for — the per-call cap settles it to that stream's own.
const LARGEST_CONFIGURABLE_RECORD_BYTES: f64 = 10.0 * 1024.0 * 1024.0;

/// The write-byte bucket's rate and burst for a stream whose records may be
/// `max_record_bytes` long: the published rate, with a second of it plus one whole record as
/// the capacity (see the module docs).
fn byte_limits(max_record_bytes: usize) -> Limits {
    #[allow(clippy::cast_precision_loss)]
    let record = (max_record_bytes as f64).min(LARGEST_CONFIGURABLE_RECORD_BYTES);
    Limits {
        rate: SHARD_WRITE_BYTES_PER_SECOND,
        burst: SHARD_WRITE_BYTES_PER_SECOND + record,
    }
}

/// What one shard has left of its write quota: the two published rates, metered side by
/// side, because a shard is bounded by BOTH ("1,000 records per second, up to a maximum
/// data write total of 1 MB per second") and a write past either of them is not taken.
#[derive(Debug, Clone, Copy)]
pub struct WriteQuota {
    /// The record-count budget, charged one record per write.
    records: Bucket,
    /// The write-byte budget, charged the bytes the record occupies.
    bytes: Bucket,
}

impl WriteQuota {
    /// A shard's quota as of `at`: both budgets full, which is what a shard nothing has been
    /// written to yet admits.
    #[must_use]
    pub fn fresh(at: Instant) -> Self {
        Self {
            records: Bucket::full(RECORD_LIMITS, at),
            // Filled to the largest capacity any configuration can ask for, so the first
            // write of a stream configured for large records is not refused for a capacity
            // the shard was laid down with; the cap in `admits` holds it to the capacity of
            // the stream as it is NOW, which `UpdateMaxRecordSize` can have changed since.
            bytes: Bucket::full(
                byte_limits(LARGEST_CONFIGURABLE_RECORD_BYTES as usize),
                at,
            ),
        }
    }

    /// Whether the shard admits a record occupying `bytes` at `at`, charging the quota for it
    /// when it does. `max_record_bytes` is what one record of the stream this shard belongs
    /// to may carry, which is what its capacity is read from ([`byte_limits`]).
    ///
    /// A record the shard does not take spends NOTHING: both budgets are asked what they
    /// hold before either is charged, so a producer hammering a saturated shard cannot drain
    /// the record rate with writes the byte rate refused (and the other way round), and the
    /// two published rates stay independent.
    pub fn admits(&mut self, bytes: usize, max_record_bytes: usize, at: Instant) -> bool {
        let limits = byte_limits(max_record_bytes);
        #[allow(clippy::cast_precision_loss)]
        let charged = (bytes as f64).min(limits.burst);
        let records_left = self.records.capacity_at(RECORD_LIMITS, at);
        let bytes_left = self.bytes.capacity_at(limits, at);
        if records_left < ONE_RECORD || bytes_left < charged {
            return false;
        }
        self.records.charge(ONE_RECORD);
        self.bytes.charge(charged);
        true
    }
}

/// What one record takes out of the record-count budget.
const ONE_RECORD: f64 = 1.0;

/// How a write states that the shard it addressed is past its write quota: the error class
/// "The request rate for the stream is too high, or the requested data is too large for the
/// available throughput" (`ProvisionedThroughputExceededException`), worded with the three
/// values AWS puts in it — "An ErrorCode value of ProvisionedThroughputExceededException has
/// an error message that includes the account ID, stream name, and shard ID"
/// (`PutRecordsResultEntry$ErrorMessage`).
///
/// Built FROM the request and the store: the shard the record was placed in, the stream it
/// belongs to, and the account serving the call.
#[must_use]
pub fn write_rate_exceeded_message(stream_name: &str, shard_id: &str) -> String {
    format!(
        "Rate exceeded for shard {shard_id} in stream {stream_name} under account {}.",
        ::engine::emulator_account_id()
    )
}

/// The `ErrorCode` a `PutRecords` entry the shard could not take carries: "ErrorCodes can be
/// either ProvisionedThroughputExceededException or InternalFailure"
/// (`PutRecordsResultEntry$ErrorCode`).
pub const THROUGHPUT_EXCEEDED_ERROR_CODE: &str = "ProvisionedThroughputExceededException";

#[cfg(test)]
mod tests {
    use super::*;

    /// The default maximum record size, in bytes: `MaxRecordSizeInKiB` falls back to 1024
    /// KiB.
    const DEFAULT_MAXIMUM: usize = 1024 * 1024;

    /// RULE over a table: a shard admits its capacity of records and of write bytes at once,
    /// and the write that would pass either published rate is not admitted — whichever of
    /// the two budgets it exhausts.
    #[test]
    fn a_shard_admits_its_capacity_of_records_and_of_bytes_and_no_more() {
        let start = Instant::now();
        // (case, the bytes each record carries, the stream's maximum record size, how many
        // of those records are admitted back to back)
        let rows: [(&str, usize, usize, usize); 5] = [
            // Tiny records: the record-count rate is what stops them.
            ("one-byte records", 1, DEFAULT_MAXIMUM, 1_000),
            // 1 KiB records: 1,000 of them is under the byte capacity, so the count rate
            // still binds first.
            ("kibibyte records", 1024, DEFAULT_MAXIMUM, 1_000),
            // 16 KiB records: the byte capacity binds first, at a second of write bytes
            // plus one record's worth.
            ("16 KiB records", 16 * 1024, DEFAULT_MAXIMUM, 128),
            // A record of exactly the stream's maximum: the capacity holds one of them with
            // a second of write bytes beside it, so two go through and the third does not.
            ("records at the maximum", DEFAULT_MAXIMUM, DEFAULT_MAXIMUM, 2),
            // The same, on a stream configured for the largest record the model admits.
            (
                "records at the configured ceiling",
                10 * 1024 * 1024,
                10 * 1024 * 1024,
                1,
            ),
        ];
        for (case, bytes, maximum, expected) in rows {
            let mut quota = WriteQuota::fresh(start);
            let mut admitted = 0usize;
            // Every write is asked at the SAME instant, so nothing refills between them and
            // the count is the capacity rather than a function of how fast the host runs.
            for _ in 0..=expected {
                if quota.admits(bytes, maximum, start) {
                    admitted += 1;
                }
            }
            assert_eq!(
                admitted, expected,
                "case {case:?}: expected {expected} records of {bytes} bytes admitted at one \
                 instant on a stream whose maximum is {maximum}, got {admitted}"
            );
        }
    }

    /// A record of the stream's whole maximum is taken on a shard nothing has been written
    /// to, however that maximum compares with the per-second rate, and the writes behind it
    /// wait for the refill.
    #[test]
    fn a_record_at_the_streams_maximum_is_taken_and_the_next_writes_wait() {
        let start = Instant::now();
        let maximum = 10 * 1024 * 1024;
        let mut quota = WriteQuota::fresh(start);
        assert!(
            quota.admits(maximum, maximum, start),
            "case a 10 MiB record: a record of the stream's maximum is taken"
        );
        assert!(
            !quota.admits(maximum, maximum, start),
            "case a second one at the same instant: the capacity it spent is not refilled yet"
        );
        // Ten seconds refills the whole rate-fed part of the capacity, which admits the
        // record again.
        let later = start + ::std::time::Duration::from_secs(10);
        assert!(
            quota.admits(maximum, maximum, later),
            "case once the rate refilled: the shard takes a record of its maximum again"
        );
    }

    /// A record the shard does not take spends NO capacity: a write the byte budget refuses
    /// leaves the record budget where it was, so a producer that keeps writing to a
    /// saturated shard cannot drain the rate its writes never used.
    #[test]
    fn a_record_the_shard_does_not_take_spends_no_capacity() {
        let start = Instant::now();
        let maximum = DEFAULT_MAXIMUM;
        let mut quota = WriteQuota::fresh(start);
        // Spend the whole byte capacity with two records at the stream's maximum, which
        // leaves the record budget all but untouched.
        assert!(
            quota.admits(maximum, maximum, start) && quota.admits(maximum, maximum, start),
            "case the capacity: expected both records at the maximum taken"
        );
        // Every further write at that instant is refused by the byte budget...
        for _ in 0..100 {
            assert!(
                !quota.admits(maximum, maximum, start),
                "case a saturated shard: expected the write refused"
            );
        }
        // ...and the record budget still holds what those refused writes did not spend, so a
        // write goes through the moment the byte rate has refilled.
        let later = start + ::std::time::Duration::from_secs(1);
        assert!(
            quota.admits(8, maximum, later),
            "case once the byte rate refilled: expected the record budget to still admit a \
             write"
        );
    }
}
