// SPDX-License-Identifier: CC0-1.0

//! Two refusals this service words from AWS's own FIXED templates, end to end over the wire:
//! a `CreateStream` whose name this account and region already hold a stream under, and a
//! `GetShardIterator` whose `ShardIteratorType` and `StartingSequenceNumber` do not go
//! together.
//!
//! RULE: an error code and an error message are part of the contract, so each refusal carries
//! the template AWS words that error CLASS with — the fixed words exactly, and every variable
//! part taken from the request and from the emulator's own state, nothing else:
//!
//! * a name this account and region already hold a stream under is the modelled
//!   `ResourceInUseException` `Stream <streamName> under account <accountId> already exists.`
//!   — the name from the request and the 12-digit account id from state, in the same spans
//!   every other "under account" sentence of this service uses. A stream name is scoped by
//!   region ("The stream name identifies the stream... It is also scoped by Amazon Web
//!   Services Region", `CreateStream`), so the SAME name in another region is served instead;
//! * an iterator type and a sequence number that do not go together is the modelled
//!   `InvalidArgumentException` enumerating the two legal combinations and then restating
//!   what the request named: `Must either specify (1) AT_SEQUENCE_NUMBER or
//!   AFTER_SEQUENCE_NUMBER and <member> or (2) TRIM_HORIZON or LATEST and no <member>.
//!   Request specified <shardIteratorType> and <no <member>|<member>>.` — one sentence for
//!   the one rule, with the member the request spells a sequence number with (this operation's
//!   `StartingSequenceNumber`; the other reader of a shard spells it
//!   `StartingPosition.SequenceNumber`, which [`crate::enhanced_fanout_subscriptions`]
//!   pins);
//! * an operation served only for an account holding a minimum throughput billing commitment
//!   refuses an account that holds none with the modelled `ValidationException` `Account
//!   <accountId> is not enabled for minimum throughput billing commitment` — the account id
//!   from state is the only variable part — and it does so BEFORE the request's stream is
//!   resolved, so the answer does not depend on whether that stream exists.
//!
//! The recorded cases this was derived from are one row each — one duplicate create, and one
//! `AT_SEQUENCE_NUMBER` read carrying no sequence number. The other rows differ along what
//! the rule generalises over: the form and length of the stream name, the capacity mode the
//! held stream was created in, the lifecycle status it is in when the second create arrives,
//! the region the create is signed for, and — for the reader — every `ShardIteratorType` the
//! model declares, each with and without a sequence number, so both the refused and the
//! served combinations are covered.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness and the ARN builder are [`crate::stream_tags`]', reused rather than
//! copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    account_not_enabled_for_commitment_message, shard_id, stream_already_exists_message,
    STREAM_MODE_ON_DEMAND,
};

use crate::stream_tags::{error_code, error_message, poll_until_active, Emulator, REGION};
use crate::warm_throughput::commit_the_account;

/// The `@length` maximum of `StreamName`, in characters
/// (`smithy_query(query="shape",shape="com.amazonaws.kinesis#StreamName")` -> len 1..128,
/// pattern `^[a-zA-Z0-9_.-]+$`).
const STREAM_NAME_MAX: usize = 128;

/// Every value `ShardIteratorType` admits
/// (`smithy_query(query="shape",shape="com.amazonaws.kinesis#ShardIteratorType")`), which is
/// the catalog the reader rows below are written over.
const SHARD_ITERATOR_TYPES: [&str; 5] = [
    "AT_SEQUENCE_NUMBER",
    "AFTER_SEQUENCE_NUMBER",
    "TRIM_HORIZON",
    "LATEST",
    "AT_TIMESTAMP",
];

/// The code and the message `operation` refuses `body` with, asserting the modelled client
/// status.
async fn refusal(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    body: &Value,
) -> (String, String) {
    let (status, response) = emulator.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected the modelled client refusal, got {status} with body {response}"
    );
    (error_code(&response), error_message(&response))
}

/// The sentence AWS words a duplicate create with, written out here rather than borrowed from
/// the crate, so the wording itself is what these cases pin.
fn already_exists(name: &str) -> String {
    format!(
        "Stream {name} under account {} already exists.",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A create whose name this account and region already hold a stream under is refused with
/// AWS's `already exists` template — whatever the name looks like, whichever capacity mode
/// either create names, and whether the held stream is still `CREATING` or already `ACTIVE` —
/// while the same name in another region is a stream of its own.
#[tokio::test]
async fn a_duplicate_create_states_the_name_and_the_account() {
    let emulator = Emulator::new();

    // (case, the stream's name, the body the first create sends, the body the second sends).
    // The names differ in length and in the punctuation the member's `@pattern` admits, and
    // the bodies differ in the capacity mode they name, so the sentence is the rule's and not
    // one shape of request's.
    let longest = "d".repeat(STREAM_NAME_MAX);
    let rows: Vec<(&str, String, Value, Value)> = vec![
        (
            "a provisioned stream created twice",
            "dup-provisioned".to_string(),
            json!({ "ShardCount": 1 }),
            json!({ "ShardCount": 1 }),
        ),
        (
            "an on-demand stream created twice",
            "dup.on_demand-stream".to_string(),
            json!({ "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND } }),
            json!({ "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND } }),
        ),
        (
            "a provisioned name asked for in the on-demand mode",
            "dup-mode-crossed".to_string(),
            json!({ "ShardCount": 2 }),
            json!({ "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND } }),
        ),
        (
            "the second create naming more shards and tags",
            "dup-richer-second-create".to_string(),
            json!({ "ShardCount": 1 }),
            json!({ "ShardCount": 3, "Tags": { "owner": "second" } }),
        ),
        (
            "the longest name the model admits",
            longest,
            json!({ "ShardCount": 1 }),
            json!({ "ShardCount": 1 }),
        ),
    ];

    for (case, name, first, _) in &rows {
        let mut body = first.clone();
        body["StreamName"] = json!(name);
        emulator.ok(case, "CreateStream", &body).await;
    }

    // While the held stream is still inside the transition its create armed, the name is
    // already taken: the refusal is about the NAME and not about the stream being ready.
    for (case, name, _, second) in &rows {
        let case = format!("{case}, while the held stream is still creating");
        let mut body = second.clone();
        body["StreamName"] = json!(name);
        let (code, message) = refusal(&emulator, &case, "CreateStream", &body).await;
        assert_eq!(
            code, "ResourceInUseException",
            "case {case}: expected the modelled conflict, got {code} with {message:?}"
        );
        assert_eq!(
            message,
            already_exists(name),
            "case {case}: expected AWS's already-exists sentence, got {message:?}"
        );
        assert_eq!(
            message,
            stream_already_exists_message(name),
            "case {case}: the crate's own sentence is the one the wire carries"
        );
    }

    // And once it is ACTIVE, the same name is refused with the same sentence. The CONDITION
    // each held stream reaches on the emulator's own clock is what is waited for, never a
    // duration this case measures.
    for (case, name, _, _) in &rows {
        poll_until_active(&emulator, case, name).await;
    }
    for (case, name, _, second) in &rows {
        let case = format!("{case}, once the held stream is active");
        let mut body = second.clone();
        body["StreamName"] = json!(name);
        let (code, message) = refusal(&emulator, &case, "CreateStream", &body).await;
        assert_eq!(
            code, "ResourceInUseException",
            "case {case}: expected the modelled conflict, got {code} with {message:?}"
        );
        assert_eq!(
            message,
            already_exists(name),
            "case {case}: expected AWS's already-exists sentence, got {message:?}"
        );
    }

    // The other half of the rule: a stream name is scoped by region, so the same name signed
    // for another region names no stream yet and the create is SERVED.
    for (case, name, first, _) in &rows {
        let mut body = first.clone();
        body["StreamName"] = json!(name);
        for region in ["us-west-2", "eu-central-1"] {
            assert_ne!(
                region, REGION,
                "case {case}: the served rows must be signed for another region"
            );
            emulator.ok_in(case, region, "CreateStream", &body).await;
        }
    }
}

/// A `GetShardIterator` whose `ShardIteratorType` and `StartingSequenceNumber` do not go
/// together is refused with AWS's two-clause template — the legal combinations enumerated,
/// then the type the request named and whether it carried a sequence number — while every
/// combination that DOES go together is served.
#[tokio::test]
async fn an_inconsistent_iterator_type_and_sequence_number_states_both_combinations() {
    let emulator = Emulator::new();
    let stream = "iterator-combinations";
    emulator
        .ok(
            "setup",
            "CreateStream",
            &json!({ "StreamName": stream, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, "setup", stream).await;

    // A record, so the sequence number the rows carry is one the shard really holds: a value
    // outside the shard's published range is a different refusal of this same error class.
    let sequence_number = emulator
        .ok(
            "setup",
            "PutRecord",
            &json!({
                "StreamName": stream,
                "PartitionKey": "pk",
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        )
        .await["SequenceNumber"]
        .as_str()
        .expect("PutRecord answers the sequence number it stored")
        .to_string();

    // Every `ShardIteratorType` the model declares, with and without a sequence number: the
    // two sequence-number types need one, the three others take none, and `AT_TIMESTAMP`
    // carries its own instant so that only the sequence number is at fault in its refused row.
    for shard_iterator_type in SHARD_ITERATOR_TYPES {
        let takes_sequence_number = shard_iterator_type == "AT_SEQUENCE_NUMBER"
            || shard_iterator_type == "AFTER_SEQUENCE_NUMBER";
        for carried_sequence_number in [false, true] {
            let case = format!(
                "{shard_iterator_type} with{} a sequence number",
                if carried_sequence_number { "" } else { "out" }
            );
            let mut body = json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": shard_iterator_type,
            });
            if carried_sequence_number {
                body["StartingSequenceNumber"] = json!(sequence_number);
            }
            if shard_iterator_type == "AT_TIMESTAMP" {
                body["Timestamp"] = json!(1_700_000_000);
            }

            if takes_sequence_number == carried_sequence_number {
                // The combination goes together, so an iterator is handed out: the refusal
                // below is about the combination and not about the member being there at all.
                let served = emulator.ok(&case, "GetShardIterator", &body).await;
                assert!(
                    served
                        .get("ShardIterator")
                        .and_then(Value::as_str)
                        .is_some_and(|iterator| !iterator.is_empty()),
                    "case {case}: a combination the type admits is served an iterator, got \
                     {served}"
                );
                continue;
            }

            let (code, message) = refusal(&emulator, &case, "GetShardIterator", &body).await;
            assert_eq!(
                code, "InvalidArgumentException",
                "case {case}: expected the modelled invalid-argument, got {code} with \
                 {message:?}"
            );
            let expected = format!(
                "Must either specify (1) AT_SEQUENCE_NUMBER or AFTER_SEQUENCE_NUMBER and \
                 StartingSequenceNumber or (2) TRIM_HORIZON or LATEST and no \
                 StartingSequenceNumber. Request specified {shard_iterator_type} and \
                 {}StartingSequenceNumber.",
                if carried_sequence_number { "" } else { "no " }
            );
            assert_eq!(
                message, expected,
                "case {case}: expected AWS's two-clause template, got {message:?}"
            );
        }
    }
}

/// `UpdateStreamWarmThroughput` is served only for an account holding a minimum throughput
/// billing commitment, and that ACCOUNT-level precondition is judged before the request's
/// stream is resolved: every request an account holding none sends is refused with the one
/// sentence, whatever stream it names and whatever else about it is wrong, and the same
/// requests are judged on their own merits once the commitment is held.
#[tokio::test]
async fn the_account_commitment_is_judged_before_the_stream_is_resolved() {
    let emulator = Emulator::new();
    let on_demand = "commitment-on-demand";
    let provisioned = "commitment-provisioned";
    emulator
        .ok(
            "setup",
            "CreateStream",
            &json!({
                "StreamName": on_demand,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    emulator
        .ok(
            "setup",
            "CreateStream",
            &json!({ "StreamName": provisioned, "ShardCount": 1 }),
        )
        .await;
    poll_until_active(&emulator, "setup", on_demand).await;
    poll_until_active(&emulator, "setup", provisioned).await;

    // (case, the request body). The rows differ in the stream they address and in what ELSE
    // the request gets wrong — a stream nothing holds, an ARN nothing holds, a stream that
    // would be served, a stream whose capacity mode does not admit the change, a request
    // naming no stream at all, and a target past the published ceiling — because an account
    // that holds no commitment is refused before any of that is reached.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a stream name nothing holds",
            json!({ "StreamName": "commitment-absent", "WarmThroughputMiBps": 1 }),
        ),
        (
            "a stream ARN nothing holds",
            json!({
                "StreamARN": format!(
                    "arn:aws:kinesis:{REGION}:{}:stream/commitment-absent-too",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "WarmThroughputMiBps": 64,
            }),
        ),
        (
            "an on-demand stream that would be served",
            json!({ "StreamName": on_demand, "WarmThroughputMiBps": 64 }),
        ),
        (
            "a stream whose capacity mode does not admit the change",
            json!({ "StreamName": provisioned, "WarmThroughputMiBps": 64 }),
        ),
        (
            "a request naming no stream at all",
            json!({ "WarmThroughputMiBps": 64 }),
        ),
        (
            "a target past the published ceiling",
            json!({ "StreamName": on_demand, "WarmThroughputMiBps": 1_000_000 }),
        ),
    ];

    for (case, body) in &rows {
        let (code, message) = refusal(&emulator, case, "UpdateStreamWarmThroughput", body).await;
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected the modelled validation refusal, got {code} with {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "Account {} is not enabled for minimum throughput billing commitment",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case}: expected AWS's account-precondition sentence, got {message:?}"
        );
        assert_eq!(
            message,
            account_not_enabled_for_commitment_message(),
            "case {case}: the crate's own sentence is the one the wire carries"
        );
    }

    // Once the account holds the commitment, the precondition is out of the way and each of
    // those requests is judged on its own: the on-demand stream is SERVED, and the other rows
    // are refused with the sentence their own fault earns instead.
    commit_the_account(&emulator).await;
    let served = emulator
        .ok(
            "an on-demand stream, with the commitment held",
            "UpdateStreamWarmThroughput",
            &json!({ "StreamName": on_demand, "WarmThroughputMiBps": 64 }),
        )
        .await;
    assert_eq!(
        served["WarmThroughput"]["TargetMiBps"],
        json!(64),
        "case an on-demand stream, with the commitment held: expected the change to be served, \
         got {served}"
    );
    let (code, message) = refusal(
        &emulator,
        "a stream name nothing holds, with the commitment held",
        "UpdateStreamWarmThroughput",
        &json!({ "StreamName": "commitment-absent", "WarmThroughputMiBps": 1 }),
    )
    .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "ResourceNotFoundException",
            format!(
                "Stream commitment-absent under account {} not found.",
                engine::EMULATOR_ACCOUNT_ID
            )
            .as_str()
        ),
        "case a stream name nothing holds, with the commitment held: expected the stream \
         lookup to be reached once the account precondition is met"
    );
}
