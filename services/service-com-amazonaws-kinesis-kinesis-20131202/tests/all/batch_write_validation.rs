// SPDX-License-Identifier: CC0-1.0

//! What a BATCH write refuses before it stores anything, and how it words it: `PutRecords`
//! -> 400 `ValidationException`, with the shard read back to show nothing was stored.
//!
//! RULE, in two halves, both of them the front-door validator's:
//!
//! 1. the per-record ceiling a stream publishes (`MaxRecordSizeInKiB`, which a read of the
//!    stream reports) bounds the LENGTH OF THE DATA BLOB alone — `PutRecordsRequestEntry$Data`
//!    is a blob with its own `@length` and `$PartitionKey` is a separate member with its own
//!    `@length` of 1..256
//!    (`smithy_query(query="shape",shape="com.amazonaws.kinesis#PutRecordsRequestEntry")`), so
//!    a blob AT the ceiling is stored whatever partition key travels with it, and one byte
//!    more fails that member's length: HTTP 400 `ValidationException`, one clause per entry at
//!    fault naming the blob where the request carried it (`records.<n>.member.data`) and the
//!    ceiling it passed, with the WHOLE call refused and no sibling entry stored;
//! 2. a clause about the `Records` list itself quotes the LIST the request carried — the
//!    elements in order, each printed as `PutRecordsRequestEntry(<member>=<value>, …)` over
//!    every member the shape declares, an unset member as `null` and the blob as the buffer it
//!    was read into — rather than a summary of its size, which is no value any caller sent.
//!
//! The recorded flows this was derived from send one batch whose largest entry sits at the
//! 1048576-byte ceiling and one batch of 501 one-byte entries, which are rows of the tables
//! below. The rows differ along what the rule generalises over: the stream's configured
//! ceiling, the length of the partition key beside the blob, which entry of the batch is at
//! fault and how many of them are, the entry count the list carries (past its maximum and
//! below its minimum), and which members an entry names.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness is [`crate::stream_tags`]', reused rather than copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    data_too_long_message, put_records_data_path, shard_id, DEFAULT_MAX_RECORD_SIZE_KI_B,
};

use crate::stream_tags::{error_code, error_message, poll_until_active, Emulator};

/// How many bytes one KiB is, the unit `MaxRecordSizeInKiB` is expressed in.
const BYTES_PER_KI_B: usize = 1024;

/// The `@length` maximum of `PutRecordsRequestEntry$PartitionKey`, in characters
/// (`smithy_query(query="shape",shape="com.amazonaws.kinesis#PutRecordsRequestEntry")` ->
/// `PartitionKey` len 1..256): the largest key an entry may carry, and the one that would
/// charge the most against the record ceiling if it counted towards it.
const PARTITION_KEY_MAX_LENGTH: usize = 256;

/// The `@length` maximum of `PutRecordsInput$Records`
/// (`smithy_query(query="operation",operation="PutRecords")` -> `Records` len 1..500).
const MAX_BATCH_RECORDS: usize = 500;

/// A `PutRecords` entry carrying `bytes` bytes of data under the partition key `key`.
fn entry(key: &str, bytes: usize) -> Value {
    json!({
        "PartitionKey": key,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; bytes]),
    })
}

/// How many records the shard `shard` of the stream `name` holds.
async fn stored_records(emulator: &Emulator, case: &str, name: &str, shard: &str) -> usize {
    let iterator = emulator
        .ok(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": name,
                "ShardId": shard,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
    let page = emulator
        .ok(
            case,
            "GetRecords",
            &json!({ "ShardIterator": iterator["ShardIterator"] }),
        )
        .await;
    page["Records"].as_array().map_or(0, Vec::len)
}

/// One row of the per-record ceiling table: the stream the batch is written to, the ceiling
/// that stream is configured for, and the partition key the entries carry.
struct Row {
    case: &'static str,
    /// `MaxRecordSizeInKiB` the create names, `None` on the row that names none and takes
    /// the documented default.
    ceiling_ki_b: Option<i64>,
    /// The partition key beside the blob: its length is what the recorded defect charged
    /// against the ceiling.
    key: String,
}

/// The rows the first half of the rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: the default ceiling, with a short key beside the blob.
        Row {
            case: "batch-ceiling-default",
            ceiling_ki_b: None,
            key: "pk-over".to_string(),
        },
        // A partition key at its own modelled maximum, which is the largest it can be and
        // still not count against the record ceiling.
        Row {
            case: "batch-ceiling-default-longest-key",
            ceiling_ki_b: None,
            key: "k".repeat(PARTITION_KEY_MAX_LENGTH),
        },
        // A stream configured for larger records: the ceiling a read reports is the ceiling
        // the write is measured against.
        Row {
            case: "batch-ceiling-raised",
            ceiling_ki_b: Some(2 * i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B)),
            key: "pk".to_string(),
        },
    ]
}

/// A data blob AT the stream's ceiling is stored whatever partition key travels with it, and
/// one byte more refuses the whole batch with the member's own length clause, storing nothing.
#[tokio::test]
async fn the_record_ceiling_bounds_the_data_blob_alone() {
    let emulator = Emulator::new();
    for row in rows() {
        let mut body = json!({ "StreamName": row.case, "ShardCount": 1 });
        if let Some(ceiling) = row.ceiling_ki_b {
            body["MaxRecordSizeInKiB"] = json!(ceiling);
        }
        emulator.ok(row.case, "CreateStream", &body).await;
    }
    for row in rows() {
        poll_until_active(&emulator, row.case, row.case).await;
    }
    let shard = shard_id(0);

    for row in rows() {
        let case = row.case;
        let ceiling = usize::try_from(
            row.ceiling_ki_b
                .unwrap_or(i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B)),
        )
        .unwrap_or(0)
            * BYTES_PER_KI_B;

        // The at-ceiling batch: the largest entry's blob is exactly the ceiling, with the
        // partition key on top of it, and four small siblings beside it.
        let at_ceiling: Vec<Value> = (0..5)
            .map(|index| {
                if index == 2 {
                    entry(&row.key, ceiling)
                } else {
                    entry(&row.key, 8)
                }
            })
            .collect();
        let answer = emulator
            .ok(
                case,
                "PutRecords",
                &json!({ "StreamName": case, "Records": at_ceiling }),
            )
            .await;
        assert_eq!(
            answer["FailedRecordCount"].as_i64(),
            Some(0),
            "case {case}: a batch whose largest blob is exactly the {ceiling}-byte ceiling is \
             stored whole, got {answer}"
        );
        assert_eq!(
            stored_records(&emulator, case, case, &shard).await,
            5,
            "case {case}: expected the five entries of the at-ceiling batch stored"
        );

        // One byte more of DATA, at two positions of the batch: every entry at fault
        // contributes its clause, and the whole call is refused.
        // (sub-case, the entry positions whose blob is one byte past the ceiling)
        let over: [(&str, Vec<usize>); 2] = [
            ("one entry past the ceiling", vec![3]),
            ("two entries past the ceiling", vec![1, 4]),
        ];
        for (sub_case, at_fault) in over {
            let records: Vec<Value> = (1..=5)
                .map(|position| {
                    if at_fault.contains(&position) {
                        entry(&row.key, ceiling + 1)
                    } else {
                        entry(&row.key, 8)
                    }
                })
                .collect();
            let (status, response) = emulator
                .call(
                    "PutRecords",
                    &json!({ "StreamName": case, "Records": records }),
                )
                .await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "ValidationException"),
                "case {case}/{sub_case}: a blob one byte past the ceiling fails that member's \
                 length, got {status} with body {response}"
            );
            let paths: Vec<String> = at_fault
                .iter()
                .map(|position| put_records_data_path(*position))
                .collect();
            assert_eq!(
                error_message(&response),
                data_too_long_message(&paths, ceiling),
                "case {case}/{sub_case}: expected one clause per entry at fault, naming the \
                 blob where the request carried it, got {response}"
            );
            assert_eq!(
                stored_records(&emulator, case, case, &shard).await,
                5,
                "case {case}/{sub_case}: the refused call stored nothing, so the shard still \
                 holds only the at-ceiling batch"
            );
        }
    }
}

/// One element of the list as a clause prints it: the shape's name over every member it
/// declares, in declaration order, the blob as the buffer it was read into and an unset
/// member as `null`.
///
/// Written out here rather than borrowed from the crate, so the form a client sees is what
/// this suite pins.
fn printed_entry(data_bytes: usize, explicit_hash_key: Option<&str>, partition_key: &str) -> String {
    format!(
        "PutRecordsRequestEntry(data=java.nio.HeapByteBuffer[pos=0 lim={data_bytes} \
         cap={data_bytes}], explicitHashKey={}, partitionKey={partition_key})",
        explicit_hash_key.unwrap_or("null")
    )
}

/// The whole sentence a clause about the `Records` list states.
fn list_length_message(printed: &[String], requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at 'records' failed to satisfy constraint: \
         Member must have length {requirement}",
        printed.join(", ")
    )
}

/// Assert that the clause `actual` is the one `expected`, reporting only the FIRST
/// difference when it is not: a quoted batch is tens of kilobytes, so a whole-string diff
/// says less than the byte the two part at.
fn assert_same_clause(case: &str, actual: &str, expected: &str) {
    if actual == expected {
        return;
    }
    let at = actual
        .bytes()
        .zip(expected.bytes())
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| actual.len().min(expected.len()));
    let from = at.saturating_sub(160);
    let excerpt = |text: &str| {
        text.get(from..(at + 160).min(text.len()))
            .unwrap_or(text)
            .to_string()
    };
    panic!(
        "case {case}: expected the clause to quote the list the request carried; it differs \
         at byte {at} (actual {} bytes, expected {} bytes)\n  actual:   \u{2026}{}\u{2026}\n  expected: \
         \u{2026}{}\u{2026}",
        actual.len(),
        expected.len(),
        excerpt(actual),
        excerpt(expected)
    );
}

/// A batch past the entry maximum is refused with the LIST the request carried quoted back,
/// and nothing of it is stored.
#[tokio::test]
async fn a_batch_past_the_entry_maximum_quotes_the_entries_it_carried() {
    let emulator = Emulator::new();
    let name = "batch-entry-count";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, name, name).await;
    let shard = shard_id(0);

    // One entry past the maximum, which is the batch such a clause is about. The rows differ
    // along which members the entries name: the recorded shape (a one-byte blob and a
    // partition key), and one where the entries differ in blob length and name an explicit
    // hash key too.
    let one_past = MAX_BATCH_RECORDS + 1;
    for case in ["uniform entries", "entries naming every member"] {
        let mut records: Vec<Value> = Vec::with_capacity(one_past);
        let mut printed: Vec<String> = Vec::with_capacity(one_past);
        for index in 0..one_past {
            let varied = case != "uniform entries";
            let data_bytes = if varied { 1 + index % 3 } else { 1 };
            let hash_key = (varied && index % 2 == 0).then(|| index.to_string());
            let partition_key = format!("pk-{index}");
            let mut record = entry(&partition_key, data_bytes);
            if let Some(hash_key) = hash_key.as_deref() {
                record["ExplicitHashKey"] = json!(hash_key);
            }
            records.push(record);
            printed.push(printed_entry(
                data_bytes,
                hash_key.as_deref(),
                &partition_key,
            ));
        }

        let (status, response) = emulator
            .call(
                "PutRecords",
                &json!({ "StreamName": name, "Records": records }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: a batch of {one_past} entries is past the modelled maximum, got \
             {status} with body {response}"
        );
        let expected = list_length_message(
            &printed,
            &format!("less than or equal to {MAX_BATCH_RECORDS}"),
        );
        assert_same_clause(case, &error_message(&response), &expected);
        assert_eq!(
            stored_records(&emulator, case, name, &shard).await,
            0,
            "case {case}: the refused call stored nothing"
        );
    }
}

/// The same clause states the list below the minimum the member admits: an empty batch is
/// quoted as the empty list it was.
#[tokio::test]
async fn a_batch_below_the_entry_minimum_quotes_the_empty_list() {
    let emulator = Emulator::new();
    let name = "batch-entry-count-empty";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, name, name).await;

    let (status, response) = emulator
        .call(
            "PutRecords",
            &json!({ "StreamName": name, "Records": [] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case an empty batch: expected the modelled refusal, got {status} with body {response}"
    );
    assert_same_clause(
        "an empty batch",
        &error_message(&response),
        &list_length_message(&[], "greater than or equal to 1"),
    );
}
