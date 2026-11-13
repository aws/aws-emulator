// SPDX-License-Identifier: CC0-1.0

//! What a log group's `kmsKeyId` does — and does NOT do — to subscription-filter fan-out,
//! end to end over the wire with the Kinesis emulator mounted beside this service, read
//! back through the Kinesis data plane (`GetShardIterator` -> `GetRecords`).
//!
//! RULE: a log group's `kmsKeyId` names the key the group's OWN stored log data is
//! encrypted with (`AssociateKmsKey`: "Associates the specified KMS key with either one log
//! group in the account, or with all stored CloudWatch Logs query insights results in the
//! account"), so it is a property of the group and of nothing else: fan-out neither reads
//! nor propagates it. A matching event is therefore handed to the destination as the same
//! gzip-framed envelope an unencrypted group's would be — the documented envelope members
//! and nothing else, each message readable exactly as it was ingested ("When log events are
//! sent to the receiving service, they are Base64 encoded and compressed with the GZIP
//! format") — the destination resource's encryption stays whatever its own API set it to,
//! and the group still reports the same `kmsKeyId` after the delivery and after a consumer
//! has read it.
//!
//! `subscription_fan_out.rs` grades what ONE envelope looks like across the pattern forms;
//! this suite grades the ENCRYPTION half of the same rule, which is what a client cannot
//! otherwise tell apart: delivered plaintext, delivered ciphertext, an envelope wrapping a
//! key, or no delivery at all from an encrypted group.
//!
//! The recorded flow this was derived from writes one batch to a CMK-encrypted group whose
//! filter matches everything and reads it off a one-shard destination stream reporting
//! `EncryptionType` `NONE`; that is one row of the table below. The rows differ along what
//! the rule generalises over: whether the group names a key at all and in which identifier
//! FORM, whether the destination stream has server-side encryption of its OWN (so the
//! stream's encryption is visibly not what makes the payload readable), and whether the
//! filter addresses the stream directly or through a logical `PutDestination` destination.
//!
//! Every case drives the real wire path (`POST /` with `X-Amz-Target:
//! <Service>.<Operation>` through `MultiServiceRouter::dispatch`), so what is asserted here
//! is what a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    ENCRYPTION_TYPE_KMS, ENCRYPTION_TYPE_NONE, ENCRYPTION_UPDATE_DELAY,
};

use crate::harness::{base_ms, REGION};
use crate::subscription_fan_out::{
    assert_envelope, past_activation, role_arn, stream_arn, Emulator, BY_LOG_STREAM,
};

/// The members the CloudWatch Logs subscription envelope carries, and the only ones it
/// carries: the message type, the owning account, the log group, the log stream, the
/// filters that selected the events and the events. Source: the subscription payload
/// `PutSubscriptionFilter` describes ("a real-time stream of log events ingested through
/// PutLogEvents ... delivered to a specific destination").
const ENVELOPE_MEMBERS: [&str; 6] = [
    "logEvents",
    "logGroup",
    "logStream",
    "messageType",
    "owner",
    "subscriptionFilters",
];

/// The `KeyId` the destination stream's own server-side encryption is turned on with, in
/// one of the forms `StartStreamEncryptionInput$KeyId` documents: "You can also use a
/// master key owned by Kinesis Data Streams by specifying the alias aws/kinesis".
const DESTINATION_KEY_ID: &str = "alias/aws/kinesis";

/// How long a case waits before treating a stream whose encryption changed as ready: past
/// the instant the change armed the `UPDATING` -> `ACTIVE` transition for.
fn past_encryption_update() -> Duration {
    ENCRYPTION_UPDATE_DELAY + Duration::from_millis(400)
}

/// A customer managed key ARN, the first form `CreateLogGroup$kmsKeyId` documents ("The
/// Amazon Resource Name (ARN) of the KMS key to use when encrypting log data").
fn cmk_key_arn() -> String {
    engine::emulator_arn("kms", REGION, "key/2f04b1c6-8f1a-4a1d-9c3e-9b0f5a7d1e22")
}

/// A SECOND customer managed key ARN, so the table spans more than one key: `kmsKeyId` takes
/// the ARN of a KMS KEY and no other identifier form (an alias ARN, a bare key id), which
/// `tests/all/kms_key_resolution.rs` pins as the refusal it is.
fn other_cmk_key_arn() -> String {
    engine::emulator_arn("kms", REGION, "key/9d7c3b51-4e62-4a0b-8f17-6c2ad3f0e415")
}

/// One row of the encryption table.
struct Row {
    /// What the case is called in every assertion it makes.
    case: &'static str,
    /// The `kmsKeyId` the log group is created with, or `None` for a group that names no
    /// key.
    group_key: Option<String>,
    /// Whether the DESTINATION stream carries server-side encryption of its own, which is
    /// the destination's business and not the group's.
    destination_encrypted: bool,
    /// Whether the filter's `destinationArn` names the stream directly or a logical
    /// `PutDestination` destination that names it.
    through_logical_destination: bool,
    /// The messages the batch carries; the filter matches everything, so every one of them
    /// is delivered.
    messages: &'static [&'static str],
}

/// The table: one row per form of the rule, spanning the group's key (absent, a key ARN,
/// another key ARN), the destination stream's own encryption and how the destination is
/// addressed.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no key on the group, plaintext destination stream",
            group_key: None,
            destination_encrypted: false,
            through_logical_destination: false,
            messages: &["baseline-marker alpha"],
        },
        Row {
            // The recorded case: a CMK-encrypted group delivers the same readable envelope
            // to a destination stream reporting EncryptionType NONE.
            case: "a key ARN on the group, plaintext destination stream",
            group_key: Some(cmk_key_arn()),
            destination_encrypted: false,
            through_logical_destination: false,
            messages: &["cmk-marker beta", "cmk-marker gamma"],
        },
        Row {
            case: "another key ARN on the group, plaintext destination stream",
            group_key: Some(other_cmk_key_arn()),
            destination_encrypted: false,
            through_logical_destination: false,
            messages: &["other-key-marker delta"],
        },
        Row {
            case: "a key ARN on the group, encrypted destination stream",
            group_key: Some(cmk_key_arn()),
            destination_encrypted: true,
            through_logical_destination: false,
            messages: &["both-encrypted-marker epsilon"],
        },
        Row {
            case: "no key on the group, encrypted destination stream",
            group_key: None,
            destination_encrypted: true,
            through_logical_destination: false,
            messages: &["stream-only-marker zeta"],
        },
        Row {
            case: "a key ARN on the group, through a logical destination",
            group_key: Some(cmk_key_arn()),
            destination_encrypted: false,
            through_logical_destination: true,
            messages: &["logical-marker eta"],
        },
    ]
}

/// The `kmsKeyId` CloudWatch Logs reports for `group`, or `None` when it reports none.
async fn group_key_of(emulator: &Emulator, case: &str, group: &str) -> Option<String> {
    let page = emulator
        .logs(
            case,
            "DescribeLogGroups",
            &json!({"logGroupNamePrefix": group}),
        )
        .await;
    let described = page["logGroups"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|described| described["logGroupName"].as_str() == Some(group))
        .unwrap_or_else(|| panic!("case {case}: the log group {group} must still exist, got {page}"));
    described
        .get("kmsKeyId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// The (`EncryptionType`, `KeyId`) the destination stream `stream` reports for ITSELF.
async fn stream_encryption_of(
    emulator: &Emulator,
    case: &str,
    stream: &str,
) -> (String, Option<String>) {
    let summary = emulator
        .kinesis(case, "DescribeStreamSummary", &json!({"StreamName": stream}))
        .await;
    let summary = &summary["StreamDescriptionSummary"];
    let encryption_type = summary["EncryptionType"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: a stream summary reports its EncryptionType, got {summary}")
        })
        .to_string();
    (
        encryption_type,
        summary
            .get("KeyId")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    )
}

/// The table: a group's `kmsKeyId` changes nothing about the fan-out — what is delivered,
/// what the destination's own encryption is, or what the group reports afterwards.
#[tokio::test]
async fn a_groups_kms_key_changes_nothing_about_what_its_filter_delivers() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every destination stream is created first and the activation span waited out once, so
    // the table does not pay it per row.
    for (index, row) in rows.iter().enumerate() {
        emulator
            .create_stream(row.case, &format!("cmk-fanout-{index}"))
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    // Likewise for the destination streams that carry encryption of their own.
    let mut any_encrypted = false;
    for (index, row) in rows.iter().enumerate() {
        if !row.destination_encrypted {
            continue;
        }
        any_encrypted = true;
        emulator
            .kinesis(
                row.case,
                "StartStreamEncryption",
                &json!({
                    "StreamName": format!("cmk-fanout-{index}"),
                    "EncryptionType": ENCRYPTION_TYPE_KMS,
                    "KeyId": DESTINATION_KEY_ID,
                }),
            )
            .await;
    }
    if any_encrypted {
        tokio::time::sleep(past_encryption_update()).await;
    }

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let stream = format!("cmk-fanout-{index}");
        let group = format!("/aws/emulator/cmk-fanout-{index}");
        let log_stream = format!("log-stream-{index}");
        let filter = format!("filter-{index}");
        let expected_stream_encryption = if row.destination_encrypted {
            ENCRYPTION_TYPE_KMS
        } else {
            ENCRYPTION_TYPE_NONE
        };

        let mut create = json!({"logGroupName": group});
        if let Some(key) = &row.group_key {
            create["kmsKeyId"] = json!(key);
        }
        emulator.logs(case, "CreateLogGroup", &create).await;
        emulator
            .logs(
                case,
                "CreateLogStream",
                &json!({"logGroupName": group, "logStreamName": log_stream}),
            )
            .await;

        // What each side reports about its own encryption BEFORE anything is delivered.
        let key_before = group_key_of(&emulator, case, &group).await;
        assert_eq!(
            key_before.as_deref(),
            row.group_key.as_deref(),
            "case {case}: the group reports the kmsKeyId it was created with, got \
             {key_before:?}"
        );
        let (encryption_before, key_id_before) = stream_encryption_of(&emulator, case, &stream).await;
        assert_eq!(
            encryption_before, expected_stream_encryption,
            "case {case}: the destination stream reports the encryption its own API set, \
             got {encryption_before} with KeyId {key_id_before:?}"
        );

        // The filter addresses the stream directly, or a logical destination that names it.
        let destination_arn = if row.through_logical_destination {
            emulator
                .logical_destination(case, &format!("cmk-destination-{index}"), &stream)
                .await
        } else {
            stream_arn(&stream)
        };
        emulator
            .logs(
                case,
                "PutSubscriptionFilter",
                &json!({
                    "logGroupName": group,
                    "filterName": filter,
                    "filterPattern": "",
                    "destinationArn": destination_arn,
                    "roleArn": role_arn(),
                    "distribution": BY_LOG_STREAM,
                }),
            )
            .await;

        let base = base_ms();
        let timestamps: Vec<i64> = (0..row.messages.len())
            .map(|offset| base + i64::try_from(offset).expect("the offset fits an i64"))
            .collect();
        let events: Vec<Value> = row
            .messages
            .iter()
            .zip(&timestamps)
            .map(|(message, timestamp)| json!({"timestamp": timestamp, "message": message}))
            .collect();
        emulator
            .logs(
                case,
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": log_stream,
                    "logEvents": events,
                }),
            )
            .await;

        // The delivery, read where a consumer reads it: one envelope for the one filter.
        let records = emulator.delivered(case, &stream).await;
        assert_eq!(
            records.len(),
            1,
            "case {case}: a group that names a key delivers like one that does not — one \
             envelope for the one filter, got {records:?}"
        );
        let (partition_key, envelope) = &records[0];
        assert_eq!(
            partition_key, &log_stream,
            "case {case}: log data grouped by log stream is keyed on the log stream, got \
             {partition_key}"
        );
        assert_envelope(
            case,
            envelope,
            &group,
            &log_stream,
            &filter,
            row.messages,
            &timestamps,
        );

        // The envelope carries the documented members and NOTHING else: no key, no
        // ciphertext wrapper.
        let mut members: Vec<&str> = envelope
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: an envelope is an object, got {envelope}"))
            .keys()
            .map(String::as_str)
            .collect();
        members.sort_unstable();
        assert_eq!(
            members,
            ENVELOPE_MEMBERS.to_vec(),
            "case {case}: expected exactly the envelope's documented members, got {envelope}"
        );

        // Every message is readable in the delivered payload itself, with no key material
        // anywhere in it: the frame a consumer decompressed IS the plaintext.
        let delivered_text = envelope.to_string();
        for message in row.messages {
            assert!(
                delivered_text.contains(message),
                "case {case}: the decompressed frame carries {message:?} in plaintext, got \
                 {delivered_text}"
            );
        }
        if let Some(key) = &row.group_key {
            assert!(
                !delivered_text.contains(key.as_str()),
                "case {case}: the group's key is not part of what is delivered, got \
                 {delivered_text}"
            );
        }

        // And neither side's encryption moved because events were delivered and read.
        let key_after = group_key_of(&emulator, case, &group).await;
        assert_eq!(
            key_after, key_before,
            "case {case}: the group still reports the kmsKeyId it held before the delivery, \
             got {key_after:?}"
        );
        let (encryption_after, key_id_after) = stream_encryption_of(&emulator, case, &stream).await;
        assert_eq!(
            (encryption_after.as_str(), key_id_after.as_deref()),
            (encryption_before.as_str(), key_id_before.as_deref()),
            "case {case}: the destination stream's own encryption is untouched by the \
             delivery, got {encryption_after} with KeyId {key_id_after:?}"
        );
        if let Some(key) = &row.group_key {
            assert_ne!(
                key_id_after.as_deref(),
                Some(key.as_str()),
                "case {case}: the group's key is never propagated to the destination stream, \
                 got KeyId {key_id_after:?}"
            );
        }
    }
}
