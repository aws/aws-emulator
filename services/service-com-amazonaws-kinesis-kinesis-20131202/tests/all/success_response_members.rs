// SPDX-License-Identifier: CC0-1.0

//! The MEMBER SET of this service's success responses, end to end over the wire:
//! `CreateStream` -> `DescribeStreamSummary`, and `PutRecord` / `PutRecords` across a
//! stream's encryption changes.
//!
//! RULE: a success response states a member exactly when the stream's stored state has a
//! value for it. Two halves of that rule are checked here:
//!
//! * a maximum record size is part of EVERY stream's configuration — "The maximum record
//!   size of a single record in kibibyte (KiB) that you can write to, and read from a
//!   stream" (`StreamDescriptionSummary$MaxRecordSizeInKiB`) — so a summary always reports
//!   one: the value the create named, else the default record size a stream that names none
//!   falls back to, which is also the number every write to the stream is measured against;
//! * the `EncryptionType` a write's answer carries is OPTIONAL and reports the encryption
//!   the write applied — "you can verify that encryption is applied by inspecting the API
//!   response from PutRecord or PutRecords" (`StartStreamEncryption`) — so it is present,
//!   naming the type, only while server-side encryption is configured on the stream, and
//!   absent for a stream that encrypts nothing. The stream's own description reports the
//!   CONFIGURATION instead ("The encryption type used",
//!   `StreamDescriptionSummary$EncryptionType`), which every stream has, so that member
//!   keeps naming `NONE` and is asserted present here.
//!
//! The recorded flows this was derived from describe ONE stream created without a maximum
//! record size and write to ONE unencrypted stream, which are one row of each table below.
//! The rows differ along what the rule generalises over: whether the create named a maximum
//! and where in the published range it sat, the capacity mode of the stream, the write path
//! (one record or a batch), how many entries the batch carries, and the encryption in force
//! when the write arrives (none, KMS, none again).
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the bodies
//! asserted here are the ones a client sees.
//!
//! The wire harness and the ARN builder are [`crate::stream_tags`]', reused rather than
//! copied.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    DEFAULT_MAX_RECORD_SIZE_KI_B, ENCRYPTION_TYPE_KMS, ENCRYPTION_TYPE_NONE,
    ENCRYPTION_UPDATE_DELAY, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::stream_tags::{
    error_code, past_activation, poll_until_active, stream_arn, Emulator, REGION,
};

/// The published bounds of `MaxRecordSizeInKiB`, in KiB
/// (`smithy_query(query="shape",shape="MaxRecordSizeInKiB")` -> range 1024..10240).
const MAX_RECORD_SIZE_BOUNDS: (i64, i64) = (1024, 10240);

/// How many bytes one KiB is, the unit `MaxRecordSizeInKiB` is expressed in.
const BYTES_PER_KI_B: usize = 1024;

/// The partition key every write of this suite carries. It is bounded by its OWN modelled
/// `@length` of 1..256 (`PutRecordInput$PartitionKey`) and is not charged against the
/// record-size ceiling, which bounds the data blob alone.
const PARTITION_KEY: &str = "pk";

/// A `KeyId` in one of the forms `StartStreamEncryptionInput$KeyId` documents: "You can
/// also use a master key owned by Kinesis Data Streams by specifying the alias
/// aws/kinesis".
const KEY_ID: &str = "alias/aws/kinesis";

/// How long a case waits before treating a stream whose encryption changed as ready.
fn past_encryption_update() -> Duration {
    ENCRYPTION_UPDATE_DELAY + Duration::from_millis(400)
}

/// The top-level member names a response carries, sorted, so a case can assert the whole
/// member set rather than one key at a time.
fn members(case: &str, response: &Value) -> Vec<String> {
    let object = response
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a response body, got {response}"));
    let mut names: Vec<String> = object.keys().cloned().collect();
    names.sort();
    names
}

/// The `StreamDescriptionSummary` of the stream named `name`.
async fn summary(emulator: &Emulator, case: &str, name: &str) -> Value {
    emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await
        .get("StreamDescriptionSummary")
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: DescribeStreamSummary must carry a summary"))
}

/// A `PutRecord` body for `arn` whose DATA blob is `data_bytes` long, with a partition key
/// on top of it: "Each data record can be up to 1 MiB in size by default" bounds the blob,
/// and `PutRecordInput$PartitionKey` carries its own `@length` of 1..256 beside it.
fn write_of(arn: &str, data_bytes: usize) -> Value {
    json!({
        "StreamARN": arn,
        "PartitionKey": PARTITION_KEY,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; data_bytes]),
    })
}

/// One row of the maximum-record-size table: what the create names, and the maximum the
/// stream is then configured for.
struct SizeRow {
    label: &'static str,
    /// `StreamModeDetails$StreamMode` the create names.
    mode: &'static str,
    /// `MaxRecordSizeInKiB` the create names, `None` on a row whose create names none.
    created_with: Option<i64>,
    /// The maximum the stream carries from then on, in KiB.
    expected: i64,
}

/// The table the summary half of the rule is checked over.
fn size_rows() -> Vec<SizeRow> {
    vec![
        // The recorded shape: a provisioned stream created without a maximum, which carries
        // the default record size.
        SizeRow {
            label: "summary-created-without-a-maximum",
            mode: STREAM_MODE_PROVISIONED,
            created_with: None,
            expected: i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B),
        },
        SizeRow {
            label: "summary-created-at-the-published-minimum",
            mode: STREAM_MODE_PROVISIONED,
            created_with: Some(MAX_RECORD_SIZE_BOUNDS.0),
            expected: MAX_RECORD_SIZE_BOUNDS.0,
        },
        SizeRow {
            label: "summary-created-between-the-published-bounds",
            mode: STREAM_MODE_PROVISIONED,
            created_with: Some(2 * MAX_RECORD_SIZE_BOUNDS.0),
            expected: 2 * MAX_RECORD_SIZE_BOUNDS.0,
        },
        SizeRow {
            label: "summary-created-at-the-published-ceiling",
            mode: STREAM_MODE_PROVISIONED,
            created_with: Some(MAX_RECORD_SIZE_BOUNDS.1),
            expected: MAX_RECORD_SIZE_BOUNDS.1,
        },
        // The shard count of an on-demand stream is the service's to manage; its maximum
        // record size is configured the same way a provisioned stream's is.
        SizeRow {
            label: "summary-on-demand-created-without-a-maximum",
            mode: STREAM_MODE_ON_DEMAND,
            created_with: None,
            expected: i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B),
        },
    ]
}

/// Every stream reports the maximum record size it is configured for, and the number it
/// reports is the one its writes are measured against.
#[tokio::test]
async fn every_summary_reports_the_maximum_record_size_the_stream_is_configured_for() {
    let emulator = Emulator::new();
    for row in size_rows() {
        let mut body = json!({
            "StreamName": row.label,
            "StreamModeDetails": { "StreamMode": row.mode },
        });
        // "For the data streams with a provisioned mode, you must specify the number of
        // shards" (`CreateStream`).
        if row.mode == STREAM_MODE_PROVISIONED {
            body["ShardCount"] = json!(1);
        }
        if let Some(created_with) = row.created_with {
            body["MaxRecordSizeInKiB"] = json!(created_with);
        }
        emulator.ok(row.label, "CreateStream", &body).await;
    }
    for row in size_rows() {
        poll_until_active(&emulator, row.label, row.label).await;
    }

    for row in size_rows() {
        let case = row.label;
        let summary = summary(&emulator, case, case).await;
        let reported = summary.get("MaxRecordSizeInKiB").and_then(Value::as_i64);
        assert_eq!(
            reported,
            Some(row.expected),
            "case {case}: expected the summary to report a maximum record size of {} KiB, \
             got {reported:?} in {summary}",
            row.expected
        );
        // The stream's own description reports its encryption CONFIGURATION, which every
        // stream has: a stream nothing encrypted reports the type NONE.
        assert_eq!(
            summary.get("EncryptionType").and_then(Value::as_str),
            Some(ENCRYPTION_TYPE_NONE),
            "case {case}: expected the summary to report the stream's configured encryption \
             type, got {summary}"
        );

        // What the summary reports is the bound the stream enforces on the DATA blob: a
        // blob of exactly that many bytes is stored whatever partition key travels with it,
        // and one byte more fails that member's length.
        let maximum = usize::try_from(row.expected).unwrap_or(0) * BYTES_PER_KI_B;
        let arn = stream_arn(REGION, case);
        let answer = emulator
            .ok(case, "PutRecord", &write_of(&arn, maximum))
            .await;
        assert!(
            answer
                .get("SequenceNumber")
                .and_then(Value::as_str)
                .is_some(),
            "case {case}: a record whose data is exactly the reported maximum ({maximum} \
             bytes) is stored, got {answer}"
        );
        let (status, response) = emulator.call("PutRecord", &write_of(&arn, maximum + 1)).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: a data blob one byte past the reported maximum is refused, got \
             {status} with body {response}"
        );
    }
}

/// One row of the write table: the path a record arrives by and how many entries it
/// carries.
struct WriteRow {
    label: &'static str,
    operation: &'static str,
    /// How many records the write carries; a single write always carries one.
    entries: usize,
    /// The members the answer carries beside an `EncryptionType`, sorted.
    base_members: &'static [&'static str],
}

/// The table the write half of the rule is checked over.
fn write_rows() -> Vec<WriteRow> {
    vec![
        // The recorded shapes: one single write, and one batch.
        WriteRow {
            label: "one-record",
            operation: "PutRecord",
            entries: 1,
            base_members: &["SequenceNumber", "ShardId"],
        },
        WriteRow {
            label: "a-batch-of-one",
            operation: "PutRecords",
            entries: 1,
            base_members: &["FailedRecordCount", "Records"],
        },
        WriteRow {
            label: "a-batch-of-several",
            operation: "PutRecords",
            entries: 4,
            base_members: &["FailedRecordCount", "Records"],
        },
    ]
}

/// The body `row` writes to the stream named `name`.
fn write_body(row: &WriteRow, name: &str) -> Value {
    let data = aws_smithy_types::base64::encode("payload");
    if row.operation == "PutRecord" {
        return json!({
            "StreamName": name,
            "PartitionKey": PARTITION_KEY,
            "Data": data,
        });
    }
    let entries: Vec<Value> = (0..row.entries)
        .map(|index| {
            json!({
                "PartitionKey": format!("{PARTITION_KEY}-{index}"),
                "Data": data,
            })
        })
        .collect();
    json!({ "StreamName": name, "Records": entries })
}

/// Turn encryption on for `name` and wait for the change to fall due.
async fn encrypt(emulator: &Emulator, case: &str, name: &str) {
    emulator
        .ok(
            case,
            "StartStreamEncryption",
            &json!({
                "StreamName": name,
                "EncryptionType": ENCRYPTION_TYPE_KMS,
                "KeyId": KEY_ID,
            }),
        )
        .await;
    tokio::time::sleep(past_encryption_update()).await;
}

/// Turn encryption off for `name` and wait for the change to fall due.
async fn decrypt(emulator: &Emulator, case: &str, name: &str) {
    emulator
        .ok(
            case,
            "StopStreamEncryption",
            &json!({
                "StreamName": name,
                "EncryptionType": ENCRYPTION_TYPE_KMS,
                "KeyId": KEY_ID,
            }),
        )
        .await;
    tokio::time::sleep(past_encryption_update()).await;
}

/// A write names an encryption type exactly while the stream it writes to encrypts — on
/// both write paths, and across the stream's encryption being turned on and off again.
#[tokio::test]
async fn a_write_names_an_encryption_type_only_while_the_stream_encrypts() {
    let emulator = Emulator::new();
    let name = "write-response-members";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    // (what the phase did to the stream, the type a write must name in it)
    let phases: [(&str, Option<&str>); 3] = [
        ("nothing-encrypted", None),
        ("encryption-on", Some(ENCRYPTION_TYPE_KMS)),
        ("encryption-off-again", None),
    ];

    for (phase, expected_type) in phases {
        match phase {
            "encryption-on" => encrypt(&emulator, phase, name).await,
            "encryption-off-again" => decrypt(&emulator, phase, name).await,
            _ => {}
        }

        for row in write_rows() {
            let case = format!("{phase}/{}", row.label);
            let answer = emulator
                .ok(&case, row.operation, &write_body(&row, name))
                .await;

            let reported = answer.get("EncryptionType").and_then(Value::as_str);
            assert_eq!(
                reported, expected_type,
                "case {case}: expected the answer to name the encryption type \
                 {expected_type:?}, got {reported:?} in {answer}"
            );

            let mut expected: Vec<String> =
                row.base_members.iter().map(|m| (*m).to_string()).collect();
            if expected_type.is_some() {
                expected.push("EncryptionType".to_string());
            }
            expected.sort();
            let actual = members(&case, &answer);
            assert_eq!(
                actual, expected,
                "case {case}: expected exactly the members {expected:?}, got {actual:?} in \
                 {answer}"
            );
        }
    }
}
