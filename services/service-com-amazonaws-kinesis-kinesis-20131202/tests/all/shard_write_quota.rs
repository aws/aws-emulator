// SPDX-License-Identifier: CC0-1.0

//! A shard's WRITE quota, end to end over the wire: `PutRecords` / `PutRecord` ->
//! `GetShardIterator` -> `GetRecords`.
//!
//! RULE: "Each shard can support writes up to 1,000 records per second, up to a maximum data
//! write total of 1 MB per second" (`PutRecords`) — a rate metered PER SHARD, on the shard
//! each record is placed in. A record the shard cannot take at that rate is NOT STORED, and
//! the two writers report it the way each of them models a failure: `PutRecords` marks that
//! ENTRY inside its 200 with `ErrorCode` `ProvisionedThroughputExceededException` and an
//! `ErrorMessage` naming the account, the stream and the shard ("An ErrorCode value of
//! ProvisionedThroughputExceededException has an error message that includes the account ID,
//! stream name, and shard ID", `PutRecordsResultEntry$ErrorMessage`), counts it in
//! `FailedRecordCount` and stores its accepted siblings all the same, while `PutRecord` — one
//! record, no per-entry result to carry — is refused with the operation's own
//! `ProvisionedThroughputExceededException`. So a producer that resends exactly the failed
//! subset writes each record once: an entry that was reported failed is absent from the
//! shard, and an accepted one is there exactly once.
//!
//! The recorded flows this was derived from drive one shard with back-to-back 500-entry
//! batches and with 10 KiB entries, which are rows of the tables below. The rows differ along
//! what the rule generalises over: which of the two published rates the batch passes (the
//! record count, the write bytes), the bytes one entry carries, how many entries the call
//! carries, how many shards the stream spreads them over, and which writer the record arrives
//! by.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness is [`crate::stream_tags`]', reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_id, write_rate_exceeded_message, SHARD_WRITE_BYTES_PER_SECOND,
    THROUGHPUT_EXCEEDED_ERROR_CODE,
};

use crate::stream_tags::{
    error_code, error_message, poll_until_active, stream_arn, Emulator, REGION,
};

/// The `@length` maximum of `PutRecordsInput$Records`
/// (`smithy_query(query="operation",operation="PutRecords")` -> `Records` len 1..500).
const MAX_BATCH_RECORDS: usize = 500;

/// One mebibyte, the unit the published write-byte rate is expressed in.
const MI_B: usize = 1024 * 1024;

/// The write bytes one shard holds at once: a second of the published rate plus one record of
/// the stream's maximum size, which is the capacity the service meters against (the shard has
/// to be able to take one whole record, and `MaxRecordSizeInKiB` defaults to 1024 KiB here).
fn write_byte_capacity(max_record_bytes: usize) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let per_second = SHARD_WRITE_BYTES_PER_SECOND as usize;
    per_second + max_record_bytes
}

/// How many entries of `entry_bytes` of data, under a partition key of `key_bytes`, one
/// shard's write-byte capacity admits at one instant.
///
/// What a record occupies is BOTH of its members: "The record size limit applies to the total
/// size of the partition key and data blob" (`PutRecords`), so the key's bytes are part of
/// what the rate is charged.
fn admitted_at_one_instant(entry_bytes: usize, key_bytes: usize) -> usize {
    write_byte_capacity(MI_B) / (entry_bytes + key_bytes)
}

/// The partition key every entry of a one-shard row carries, so every entry of the batch maps
/// to the one shard whose quota the row is about.
const ONE_SHARD_KEY: &str = "one-shard";

/// A `PutRecords` entry of `bytes` bytes of data under the partition key `key`.
fn entry(key: &str, bytes: usize) -> Value {
    json!({
        "PartitionKey": key,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; bytes]),
    })
}

/// The result entries of a `PutRecords` answer.
fn results<'a>(case: &str, answer: &'a Value) -> &'a Vec<Value> {
    answer["Records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: PutRecords answers one result per entry, got {answer}"))
}

/// How many entries of an answer were stored, and the sequence numbers they were stored
/// under — with every failed entry checked against the rule: it carries the modelled error
/// code, the message naming account, stream and shard, and NO sequence number or shard id.
fn accepted_sequence_numbers(
    case: &str,
    answer: &Value,
    stream: &str,
    shard: &str,
) -> Vec<String> {
    let mut accepted = Vec::new();
    for (index, result) in results(case, answer).iter().enumerate() {
        match result["ErrorCode"].as_str() {
            None => {
                let sequence_number = result["SequenceNumber"].as_str().unwrap_or_else(|| {
                    panic!("case {case}: a stored entry names its sequence number, entry {index} is {result}")
                });
                assert_eq!(
                    result["ShardId"].as_str(),
                    Some(shard),
                    "case {case}: a stored entry names the shard it went to, entry {index} is \
                     {result}"
                );
                accepted.push(sequence_number.to_string());
            }
            Some(code) => {
                assert_eq!(
                    code, THROUGHPUT_EXCEEDED_ERROR_CODE,
                    "case {case}: an entry the shard could not take carries the modelled \
                     throughput code, entry {index} is {result}"
                );
                assert_eq!(
                    result["ErrorMessage"].as_str(),
                    Some(write_rate_exceeded_message(stream, shard).as_str()),
                    "case {case}: expected the per-entry message naming the account, the \
                     stream and the shard, entry {index} is {result}"
                );
                assert!(
                    result["SequenceNumber"].is_null() && result["ShardId"].is_null(),
                    "case {case}: an entry that was not stored names no position, entry \
                     {index} is {result}"
                );
            }
        }
    }
    let reported = answer["FailedRecordCount"].as_i64().unwrap_or(-1);
    let failed = results(case, answer).len() - accepted.len();
    assert_eq!(
        reported,
        failed as i64,
        "case {case}: expected FailedRecordCount to count the entries that were not stored \
         ({failed}), got {reported}"
    );
    accepted
}

/// Every record the shard `shard` of `stream` holds, read from the trim horizon through as
/// many pages as it takes, as (sequence number, data length) pairs.
async fn stored_records(emulator: &Emulator, case: &str, stream: &str, shard: &str) -> Vec<String> {
    let iterator = emulator
        .ok(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
    let mut iterator = iterator["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetShardIterator answers an iterator"))
        .to_string();
    let mut stored: Vec<String> = Vec::new();
    // Bounded by the records one shard of this emulator retains, so a reader that stops
    // making progress ends rather than looping forever.
    for _ in 0..64 {
        let page = emulator
            .ok(case, "GetRecords", &json!({ "ShardIterator": iterator }))
            .await;
        let records = page["Records"].as_array().cloned().unwrap_or_default();
        let read = records.len();
        stored.extend(
            records
                .iter()
                .filter_map(|record| record["SequenceNumber"].as_str().map(str::to_string)),
        );
        match page["NextShardIterator"].as_str() {
            Some(next) if read > 0 => iterator = next.to_string(),
            _ => break,
        }
    }
    stored
}

/// One row of the quota table: the batch a call carries, and what the shard admits of it.
struct Row {
    case: &'static str,
    /// How many entries the call carries.
    entries: usize,
    /// The bytes of data each entry carries.
    entry_bytes: usize,
    /// How many of those entries the shard's capacity admits at one instant — `None` for a
    /// row the whole batch fits in.
    admitted: Option<usize>,
}

/// The rows the rule is checked over, each written to its OWN one-shard stream so a row is
/// judged against a shard nothing else has written to. Every entry of a row goes to the same
/// shard, so the quota under test is one shard's.
fn rows() -> Vec<Row> {
    vec![
        // Inside both published rates: a full batch of tiny entries is stored whole.
        Row {
            case: "quota-tiny-batch-inside-both-rates",
            entries: MAX_BATCH_RECORDS,
            entry_bytes: 8,
            admitted: None,
        },
        // Past the write-BYTE rate: 300 entries of 16 KiB is 4.6 MiB, more than twice what
        // the shard holds, so the call is PARTIALLY failed at the capacity.
        Row {
            case: "quota-16-kib-entries-past-the-byte-rate",
            entries: 300,
            entry_bytes: 16 * 1024,
            admitted: Some(admitted_at_one_instant(16 * 1024, ONE_SHARD_KEY.len())),
        },
        // The same rate, reached with fewer and larger entries: four 1 MiB entries are twice
        // the capacity, so what fits goes in and the rest is reported.
        Row {
            case: "quota-mebibyte-entries-past-the-byte-rate",
            entries: 4,
            entry_bytes: MI_B,
            admitted: Some(admitted_at_one_instant(MI_B, ONE_SHARD_KEY.len())),
        },
    ]
}

/// A batch past the shard's write quota is PARTIALLY failed: the entries the shard admits are
/// stored exactly once, the rest carry the modelled per-entry refusal and are absent from the
/// shard.
#[tokio::test]
async fn a_batch_past_a_shards_write_quota_is_partially_failed() {
    let emulator = Emulator::new();
    for row in rows() {
        emulator
            .ok(
                row.case,
                "CreateStream",
                &json!({ "StreamName": row.case, "ShardCount": 1 }),
            )
            .await;
    }
    for row in rows() {
        poll_until_active(&emulator, row.case, row.case).await;
    }

    let shard = shard_id(0);
    for row in rows() {
        let case = row.case;
        // One partition key for the whole batch, so every entry maps to the one shard whose
        // quota the row is about.
        let records: Vec<Value> = (0..row.entries)
            .map(|_| entry(ONE_SHARD_KEY, row.entry_bytes))
            .collect();
        let answer = emulator
            .ok(
                case,
                "PutRecords",
                &json!({ "StreamName": case, "Records": records }),
            )
            .await;
        let accepted = accepted_sequence_numbers(case, &answer, case, &shard);
        match row.admitted {
            None => assert_eq!(
                accepted.len(),
                row.entries,
                "case {case}: a batch inside both published rates is stored whole, got {} of \
                 {} entries stored",
                accepted.len(),
                row.entries
            ),
            Some(admitted) => {
                assert_eq!(
                    accepted.len(),
                    admitted,
                    "case {case}: expected the shard to admit the {admitted} entries its \
                     capacity holds and to report the rest, got {} stored",
                    accepted.len()
                );
                assert!(
                    accepted.len() < row.entries,
                    "case {case}: expected a PARTIAL outcome, got every entry stored"
                );
            }
        }

        // The shard holds exactly the accepted records, each once: nothing the call reported
        // as failed is there, and nothing is there twice.
        let stored = stored_records(&emulator, case, case, &shard).await;
        assert_eq!(
            stored, accepted,
            "case {case}: expected the shard to hold exactly the accepted records, in write \
             order; it holds {} records and {} were accepted",
            stored.len(),
            accepted.len()
        );
    }
}

/// The failed subset is what a producer resends, and resending it stores each of those
/// records exactly once: the refused entries were never stored, so the shard ends up holding
/// every record of the batch and no duplicate.
#[tokio::test]
async fn resending_the_failed_subset_stores_each_record_exactly_once() {
    let emulator = Emulator::new();
    let name = "quota-resend-failed-subset";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, name, name).await;
    let shard = shard_id(0);

    // 1 MiB entries, so the first call is partially failed at the shard's capacity whatever
    // the host's speed: the capacity holds two of them and the call carries four.
    let entries = 4usize;
    let records: Vec<Value> = (0..entries).map(|_| entry(ONE_SHARD_KEY, MI_B)).collect();
    let answer = emulator
        .ok(
            name,
            "PutRecords",
            &json!({ "StreamName": name, "Records": records.clone() }),
        )
        .await;
    let first = accepted_sequence_numbers("first call", &answer, name, &shard);
    assert!(
        !first.is_empty() && first.len() < entries,
        "case first call: expected a PARTIAL outcome, got {} of {entries} entries stored",
        first.len()
    );

    // The failed subset, resent until the shard takes it — the loop a producer runs, which
    // waits for the shard's rate to refill rather than for a fixed time.
    let mut outstanding = entries - first.len();
    let mut accepted_total = first.len();
    // The loop a producer runs: look again until the shard has taken every record of the
    // failed subset, or a generous deadline passes. What it waits for is the capacity the
    // shard's own rate refills, never a duration this case measures.
    let deadline = std::time::Instant::now() + crate::stream_tags::POLL_DEADLINE;
    while outstanding > 0 {
        let resend: Vec<Value> = (0..outstanding).map(|_| entry(ONE_SHARD_KEY, MI_B)).collect();
        let answer = emulator
            .ok(
                name,
                "PutRecords",
                &json!({ "StreamName": name, "Records": resend }),
            )
            .await;
        let accepted = accepted_sequence_numbers("resend", &answer, name, &shard);
        accepted_total += accepted.len();
        outstanding -= accepted.len();
        if outstanding > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "case resend: the shard never took the failed subset; {outstanding} entries \
                 are still outstanding"
            );
            tokio::time::sleep(crate::stream_tags::BETWEEN_LOOKS).await;
        }
    }
    assert_eq!(
        outstanding, 0,
        "case resend: expected the shard to take the failed subset once its rate refilled, \
         {outstanding} entries are still outstanding"
    );
    assert_eq!(
        accepted_total, entries,
        "case resend: expected exactly the {entries} records of the batch to be stored, \
         {accepted_total} were accepted"
    );
    let stored = stored_records(&emulator, "resend", name, &shard).await;
    assert_eq!(
        stored.len(),
        entries,
        "case resend: expected the shard to hold each record exactly once, it holds {} of \
         {entries}",
        stored.len()
    );
}

/// `PutRecord` carries no per-entry result, so the shard's quota is stated on the CALL: the
/// modelled `ProvisionedThroughputExceededException`, in the same sentence a failed batch
/// entry carries, and nothing is stored.
#[tokio::test]
async fn put_record_is_refused_with_the_modelled_throughput_error() {
    let emulator = Emulator::new();
    let name = "quota-put-record";
    // A stream configured for the largest record the model admits, so one record spends ten
    // seconds of the shard's write rate: the refusal of the SECOND such record does not
    // depend on how fast the host runs.
    let largest = 10 * 1024;
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({
                "StreamName": name,
                "ShardCount": 1,
                "MaxRecordSizeInKiB": largest,
            }),
        )
        .await;
    poll_until_active(&emulator, name, name).await;
    let shard = shard_id(0);
    let record = json!({
        "StreamARN": stream_arn(REGION, name),
        "PartitionKey": ONE_SHARD_KEY,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; largest as usize * 1024]),
    });

    let answer = emulator.ok("first record", "PutRecord", &record).await;
    let stored_at = answer["SequenceNumber"]
        .as_str()
        .unwrap_or_else(|| panic!("case first record: expected it stored, got {answer}"))
        .to_string();

    let (status, response) = emulator.call("PutRecord", &record).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, THROUGHPUT_EXCEEDED_ERROR_CODE),
        "case second record: expected the modelled throughput refusal, got {status} with \
         body {response}"
    );
    assert_eq!(
        error_message(&response),
        write_rate_exceeded_message(name, &shard),
        "case second record: expected the sentence naming the account, the stream and the \
         shard, got {response}"
    );

    // The refused write stored nothing: the shard still holds the one record it took.
    let stored = stored_records(&emulator, "second record", name, &shard).await;
    assert_eq!(
        stored,
        vec![stored_at],
        "case second record: expected the shard to hold only the record it accepted, got \
         {stored:?}"
    );
}

/// The quota is PER SHARD: a batch spread over several shards is metered against each of
/// them, so a stream with more shards takes a batch one shard could not.
#[tokio::test]
async fn the_quota_is_metered_on_each_shard_of_the_stream() {
    let emulator = Emulator::new();
    /// The entries one call carries, and the bytes each of them holds: 300 entries of 16 KiB
    /// is 4.6 MiB, more than twice what ONE shard holds and well inside what four do.
    const ENTRIES: usize = 300;
    const ENTRY_BYTES: usize = 16 * 1024;
    /// A fixed-width partition key, so every entry of the batch occupies the same bytes
    /// whatever its position.
    const KEY_WIDTH: usize = "pk-000".len();

    // (case, the shard count, how many of those entries the stream takes at one instant)
    let rows: [(&str, i64, usize); 2] = [
        (
            "quota-per-shard-one",
            1,
            admitted_at_one_instant(ENTRY_BYTES, KEY_WIDTH),
        ),
        ("quota-per-shard-four", 4, ENTRIES),
    ];
    for (name, shards, _) in rows {
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": shards }),
            )
            .await;
    }
    for (name, _, _) in rows {
        poll_until_active(&emulator, name, name).await;
    }

    for (name, shards, expected) in rows {
        // An explicit hash key per entry, stepped evenly over the 128-bit key space, so the
        // batch is spread across every shard of the stream rather than placed by one
        // partition key's hash.
        let step = u128::MAX / u128::try_from(ENTRIES).unwrap_or(1);
        let records: Vec<Value> = (0..ENTRIES)
            .map(|index| {
                let hash_key = step * u128::try_from(index).unwrap_or(0);
                json!({
                    "PartitionKey": format!("pk-{index:03}"),
                    "ExplicitHashKey": hash_key.to_string(),
                    "Data": aws_smithy_types::base64::encode(vec![b'x'; ENTRY_BYTES]),
                })
            })
            .collect();
        let answer = emulator
            .ok(
                name,
                "PutRecords",
                &json!({ "StreamName": name, "Records": records }),
            )
            .await;
        let stored = results(name, &answer)
            .iter()
            .filter(|result| result["ErrorCode"].is_null())
            .count();
        assert_eq!(
            stored, expected,
            "case {name}: expected {shards} shard(s) to admit {expected} of the {ENTRIES} \
             entries of {ENTRY_BYTES} bytes at one instant, got {stored} stored"
        );
    }
}
