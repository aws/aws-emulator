// SPDX-License-Identifier: CC0-1.0

//! A stream's KMS key on the DATA plane, end to end over the wire, with KMS mounted beside
//! Kinesis on one engine exactly as the assembled emulator mounts it: `StartStreamEncryption`
//! -> `PutRecord` / `PutRecords` / `GetRecords`, across a `DisableKey` and an `EnableKey`.
//!
//! RULE: the records of a stream encrypted under a customer managed CMK are wrapped with that
//! key, so every data-plane call RESOLVES the key against KMS and is served only while KMS
//! reports it usable:
//!
//! * a record read back through `GetRecords` reports the `EncryptionType` it was stored
//!   under, which is the stream's at the instant of the write ("You can verify that
//!   encryption is applied by inspecting the API response from PutRecord or PutRecords",
//!   `StartStreamEncryption`);
//! * while the CMK is DISABLED, `PutRecord`, `PutRecords` and `GetRecords` are each refused
//!   HTTP 400 `KMSDisabledException` — the error all three model for exactly this ("The
//!   request was rejected because the specified customer master key (CMK) isn't enabled") —
//!   with the sentence KMS itself refuses a disabled key with, `<key ARN> is disabled.`, whose
//!   only variable part is the ARN of the key the STREAM is encrypted under, whatever
//!   identifier form the encryption named it by;
//! * `StartStreamEncryption` with that disabled key is refused 400 `InvalidArgumentException`
//!   with the fixed sentence `Key is not enabled.`, so the stream keeps the configuration it
//!   had;
//! * re-enabling the key makes the identical calls succeed again, and the shard holds exactly
//!   the records that were accepted — nothing a refusal answered was stored.
//!
//! The recorded flow this was derived from encrypts ONE stream under a CMK named by its key
//! ARN, reads a record back, disables the key and polls `PutRecord`, which is one row of the
//! table below. The rows differ along what the rule generalises over: the FORM the encryption
//! named the key by (key id, key ARN), and which data-plane operation the refusal is asked of
//! (the two writers and the reader).
//!
//! The wire harness is [`crate::encryption_key_resolution`]', reused rather than copied: the
//! same two services over one engine.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    key_disabled_message, shard_id, ENCRYPTION_TYPE_KMS, KEY_NOT_ENABLED_MESSAGE, STREAM_ACTIVE,
};

use crate::encryption_key_resolution::{Emulator, KMS_TARGET_PREFIX};
use crate::stream_tags::{error_code, error_message, poll_until};

/// Wait until `name` reports `ACTIVE` with `EncryptionType` `KMS`, which is when the
/// encryption the request accepted has settled on the stream — through the crate's one poll
/// loop ([`poll_until`]).
async fn poll_until_encrypted(emulator: &Emulator, case: &str, name: &str) {
    poll_until(
        case,
        &format!("the stream {name} reports {STREAM_ACTIVE} with EncryptionType {ENCRYPTION_TYPE_KMS}"),
        || async {
            let summary = emulator
                .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
                .await;
            let summary = &summary["StreamDescriptionSummary"];
            summary["StreamStatus"].as_str() == Some(STREAM_ACTIVE)
                && summary["EncryptionType"].as_str() == Some(ENCRYPTION_TYPE_KMS)
        },
    )
    .await;
}

/// One `PutRecord` body for `name`.
fn write_of(name: &str, payload: &str) -> Value {
    json!({
        "StreamName": name,
        "PartitionKey": "one-shard",
        "Data": aws_smithy_types::base64::encode(payload),
    })
}

/// One `PutRecords` body for `name`, carrying one entry.
fn batch_of(name: &str, payload: &str) -> Value {
    json!({
        "StreamName": name,
        "Records": [{
            "PartitionKey": "one-shard",
            "Data": aws_smithy_types::base64::encode(payload),
        }],
    })
}

/// A fresh `TRIM_HORIZON` iterator over the stream's only shard.
async fn horizon_iterator(emulator: &Emulator, case: &str, name: &str) -> String {
    let answer = emulator
        .ok(
            case,
            "GetShardIterator",
            &json!({
                "StreamName": name,
                "ShardId": shard_id(0),
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
    answer["ShardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetShardIterator answers an iterator"))
        .to_string()
}

/// The records the shard holds, as (payload, encryption type) pairs.
async fn read_back(emulator: &Emulator, case: &str, name: &str) -> Vec<(String, String)> {
    let iterator = horizon_iterator(emulator, case, name).await;
    let page = emulator
        .ok(case, "GetRecords", &json!({ "ShardIterator": iterator }))
        .await;
    page["Records"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|record| {
            let data = record["Data"].as_str().unwrap_or_default();
            let decoded = aws_smithy_types::base64::decode(data).unwrap_or_default();
            (
                String::from_utf8(decoded).unwrap_or_default(),
                record["EncryptionType"]
                    .as_str()
                    .unwrap_or("<none>")
                    .to_string(),
            )
        })
        .collect()
}

/// Set the key's state through the mounted KMS.
async fn set_key_enabled(emulator: &Emulator, case: &str, key_id: &str, enabled: bool) {
    let operation = if enabled { "EnableKey" } else { "DisableKey" };
    let (status, response) = emulator
        .send(KMS_TARGET_PREFIX, operation, &json!({ "KeyId": key_id }))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: {operation} must answer 200, got {status} with body {response}"
    );
}

/// The whole contract, over both forms the encryption may name the key by.
#[tokio::test]
async fn a_disabled_cmk_blocks_the_data_plane_until_it_is_re_enabled() {
    let emulator = Emulator::new();
    // (case, whether the encryption names the key by its ARN rather than by its key id)
    let rows: [(&str, bool); 2] = [("cmk-named-by-key-id", false), ("cmk-named-by-key-arn", true)];

    for (case, by_arn) in rows {
        emulator.active_stream(case, case).await;
        let (key_id, key_arn) = emulator.create_key().await;
        let named_by = if by_arn { &key_arn } else { &key_id };
        emulator
            .ok(
                case,
                "StartStreamEncryption",
                &json!({
                    "StreamName": case,
                    "EncryptionType": ENCRYPTION_TYPE_KMS,
                    "KeyId": named_by,
                }),
            )
            .await;
        poll_until_encrypted(&emulator, case, case).await;

        // The positive control: a record written under the enabled key is stored and reads
        // back reporting the encryption it was stored under.
        let answer = emulator.ok(case, "PutRecord", &write_of(case, "first")).await;
        assert_eq!(
            answer["EncryptionType"].as_str(),
            Some(ENCRYPTION_TYPE_KMS),
            "case {case}: a write to an encrypted stream reports the encryption it applied, \
             got {answer}"
        );
        assert_eq!(
            read_back(&emulator, case, case).await,
            vec![("first".to_string(), ENCRYPTION_TYPE_KMS.to_string())],
            "case {case}: expected the record read back reporting EncryptionType \
             {ENCRYPTION_TYPE_KMS}"
        );

        // With the CMK disabled, every data-plane call is refused — the two writers and the
        // reader — in KMS's own wording for a disabled key, naming the key's ARN.
        set_key_enabled(&emulator, case, &key_id, false).await;
        let iterator = horizon_iterator(&emulator, case, case).await;
        let refused: [(&str, Value); 3] = [
            ("PutRecord", write_of(case, "while-disabled")),
            ("PutRecords", batch_of(case, "while-disabled")),
            ("GetRecords", json!({ "ShardIterator": iterator })),
        ];
        for (operation, body) in refused {
            let (status, response) = emulator.call(operation, &body).await;
            assert_eq!(
                (status, error_code(&response).as_str()),
                (400, "KMSDisabledException"),
                "case {case}/{operation}: expected the modelled refusal while the CMK is \
                 disabled, got {status} with body {response}"
            );
            assert_eq!(
                error_message(&response),
                key_disabled_message(&key_arn),
                "case {case}/{operation}: expected the sentence naming the key's ARN, got \
                 {response}"
            );
        }

        // Applying that key to a stream is refused too, in the sentence the control plane
        // states for a key that is not enabled.
        let (status, response) = emulator
            .call(
                "StartStreamEncryption",
                &json!({
                    "StreamName": case,
                    "EncryptionType": ENCRYPTION_TYPE_KMS,
                    "KeyId": named_by,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidArgumentException"),
            "case {case}: expected the control plane to refuse a key that is not enabled, got \
             {status} with body {response}"
        );
        assert_eq!(
            error_message(&response),
            KEY_NOT_ENABLED_MESSAGE,
            "case {case}: expected the fixed sentence for a key that is not enabled, got \
             {response}"
        );

        // Nothing the refusals answered was stored: the shard still holds the one record it
        // took.
        set_key_enabled(&emulator, case, &key_id, true).await;
        assert_eq!(
            read_back(&emulator, case, case).await,
            vec![("first".to_string(), ENCRYPTION_TYPE_KMS.to_string())],
            "case {case}: expected the shard to hold exactly the accepted record"
        );

        // And the identical calls are served again.
        emulator
            .ok(case, "PutRecord", &write_of(case, "re-enabled"))
            .await;
        emulator
            .ok(case, "PutRecords", &batch_of(case, "batched"))
            .await;
        assert_eq!(
            read_back(&emulator, case, case).await,
            vec![
                ("first".to_string(), ENCRYPTION_TYPE_KMS.to_string()),
                ("re-enabled".to_string(), ENCRYPTION_TYPE_KMS.to_string()),
                ("batched".to_string(), ENCRYPTION_TYPE_KMS.to_string()),
            ],
            "case {case}: expected the records accepted after the key was re-enabled, in \
             write order"
        );
    }
}

/// A stream nothing encrypted is untouched by the key state of any CMK: its data plane has no
/// key to resolve, so it serves while a key of the account is disabled.
#[tokio::test]
async fn an_unencrypted_stream_is_unaffected_by_a_disabled_key() {
    let emulator = Emulator::new();
    let case = "cmk-unencrypted-stream";
    emulator.active_stream(case, case).await;
    let (key_id, _) = emulator.create_key().await;
    set_key_enabled(&emulator, case, &key_id, false).await;

    emulator.ok(case, "PutRecord", &write_of(case, "plain")).await;
    let read = read_back(&emulator, case, case).await;
    assert_eq!(
        read.len(),
        1,
        "case {case}: expected the write to an unencrypted stream to be served, got {read:?}"
    );
    assert_ne!(
        read.first().map(|(_, encryption)| encryption.as_str()),
        Some(ENCRYPTION_TYPE_KMS),
        "case {case}: a record of an unencrypted stream reports no KMS encryption, got \
         {read:?}"
    );
}
