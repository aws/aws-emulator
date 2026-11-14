// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusals its handlers answer, end to end over the wire: a dry
//! run, a stream or a shard that is not there, a `StreamName` / `StreamARN` pair whose halves
//! disagree, an on-demand create that names a shard count, and an `ExplicitHashKey` outside
//! the key space.
//!
//! RULE: an error code and an error message are part of the contract, so each of these
//! refusals is worded the way the service words that error CLASS — fixed text, with every
//! variable part taken from the request and from the emulator's own state:
//!
//! * a dry run says the request would have SUCCEEDED and names the operation that judged it,
//!   and stores nothing;
//! * a stream nothing holds is `Stream <name> under account <account> not found.`; a read
//!   that asked for a SHARD is worded at the shard, in the sentence its CONDITION earns —
//!   `Shard <shardId> in stream <name> under account <account> does not exist` for a stream
//!   this account does not hold at all, `Could not find shard <shardId> in stream <name>
//!   under account <account>.` for a shard the HELD stream's map does not carry; and a
//!   listing narrowed by a creation instant names that instant too;
//! * a pair of identifiers that disagree names BOTH halves, since either one may be the
//!   wrong one, and the pair is reconciled from the request before any lookup;
//! * a shard count named beside the on-demand capacity mode is refused with one fixed
//!   sentence;
//! * a hash key past the 128-bit key space is a RANGE violation of the member that carried
//!   it, which is a different fault from a key the shard map does not cover.
//!
//! The recorded cases this was derived from are one row each; the other rows differ along
//! what the rule generalises over — which operation is called, which identifier addresses
//! the stream, which shard is asked for, how far past the key space the value is, and how
//! many shards the stream has.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    dry_run_succeeded_message, hash_key_out_of_range_message, shard_id,
    shard_not_found_message, shard_not_in_stream_message, stream_creation_time_not_found_message,
    stream_identifier_mismatch_message, stream_not_found_message, ON_DEMAND_SHARD_COUNT_MESSAGE,
    SHARD_ITERATOR_TYPE_TRIM_HORIZON, STREAM_MODE_ON_DEMAND,
};

use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// The largest hash key a stream's shard map covers: `2^128 - 1`, the last 128-bit integer.
const LAST_HASH_KEY: &str = "340282366920938463463374607431768211455";

/// One past it — the value the recorded case sends — which no shard map can cover because it
/// is not a 128-bit integer at all.
const PAST_THE_KEY_SPACE: &str = "340282366920938463463374607431768211456";

/// The code and the message `operation` refuses `body` with.
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

/// An `ACTIVE` provisioned stream of `shards` shards. Answers its ARN.
async fn active_stream(emulator: &Emulator, name: &str, shards: i64) -> String {
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": shards }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    stream_arn(REGION, name)
}

/// A dry run of a data-plane call is refused with the error the model declares for it,
/// stating that the request WOULD have succeeded and naming the operation that judged it —
/// and the stream is left exactly as it was.
#[tokio::test]
async fn a_dry_run_says_the_request_would_have_succeeded_and_names_the_operation() {
    let emulator = Emulator::new();
    let stream = "dry-run-wording";
    active_stream(&emulator, stream, 1).await;

    // A record to read, so the dry runs below judge requests that really would have been
    // served, and an iterator to hand `GetRecords`.
    emulator
        .ok(
            "seed",
            "PutRecord",
            &json!({
                "StreamName": stream,
                "PartitionKey": "pk",
                "Data": aws_smithy_types::base64::encode("first"),
            }),
        )
        .await;
    let iterator = emulator
        .ok(
            "seed",
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await["ShardIterator"]
        .as_str()
        .expect("GetShardIterator answers an iterator")
        .to_string();

    // Every operation whose input models the member, each with a request that would have
    // been served.
    let rows: Vec<(&str, Value)> = vec![
        (
            "PutRecord",
            json!({
                "StreamName": stream,
                "PartitionKey": "pk",
                "Data": aws_smithy_types::base64::encode("dry"),
                "DryRun": true,
            }),
        ),
        (
            "PutRecords",
            json!({
                "StreamName": stream,
                "Records": [{
                    "PartitionKey": "pk",
                    "Data": aws_smithy_types::base64::encode("dry"),
                }],
                "DryRun": true,
            }),
        ),
        (
            "GetShardIterator",
            json!({
                "StreamName": stream,
                "ShardId": shard_id(0),
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
                "DryRun": true,
            }),
        ),
        (
            "GetRecords",
            json!({ "ShardIterator": iterator.clone(), "DryRun": true }),
        ),
    ];

    for (operation, body) in rows {
        let (code, message) = refusal(&emulator, operation, operation, &body).await;
        assert_eq!(
            code, "DryRunOperationException",
            "case {operation}: expected the modelled dry-run refusal, got {code} with \
             message {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "DryRunOperation validation succeeded while calling {operation} operation.: \
                 Request would have succeeded, but DryRun flag is set."
            ),
            "case {operation}: expected AWS's dry-run sentence naming the operation"
        );
        assert_eq!(
            message,
            dry_run_succeeded_message(operation),
            "case {operation}: the crate's own sentence is the one the wire carries"
        );
    }

    // Nothing the dry runs judged was stored: the shard still holds the one seeded record,
    // and the iterator the reads were refused with still answers it.
    let page = emulator
        .ok(
            "after",
            "GetRecords",
            &json!({ "ShardIterator": iterator }),
        )
        .await;
    assert_eq!(
        page["Records"].as_array().map(Vec::len),
        Some(1),
        "a dry run stores no record, so the shard still holds the one seeded record, got \
         {page}"
    );
}

/// A refusal about something that is not there names what was missing: a stream by name and
/// account, a shard by shard id, stream and account, and a listing narrowed by a creation
/// instant by that instant too.
#[tokio::test]
async fn a_missing_resource_is_worded_at_what_was_asked_for() {
    let emulator = Emulator::new();
    let held = "missing-resource-held";
    let arn = active_stream(&emulator, held, 2).await;
    let absent = "missing-resource-absent";

    // A write names the STREAM that is not there, whichever identifier addressed it.
    let writes: Vec<(&str, &str, Value)> = vec![
        (
            "PutRecord by name",
            absent,
            json!({
                "StreamName": absent,
                "PartitionKey": "pk",
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        ),
        (
            "PutRecord by ARN",
            absent,
            json!({
                "StreamARN": stream_arn(REGION, absent),
                "PartitionKey": "pk",
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        ),
        (
            "PutRecords by name",
            absent,
            json!({
                "StreamName": absent,
                "Records": [{
                    "PartitionKey": "pk",
                    "Data": aws_smithy_types::base64::encode("x"),
                }],
            }),
        ),
        (
            "ListShards by name",
            absent,
            json!({ "StreamName": absent }),
        ),
    ];
    for (case, name, body) in writes {
        let operation = case.split(' ').next().expect("the case names an operation");
        let (code, message) = refusal(&emulator, case, operation, &body).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected the modelled not-found, got {code} with {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "Stream {name} under account {} not found.",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {case}: expected the stream-not-found sentence"
        );
        assert_eq!(
            message,
            stream_not_found_message(name),
            "case {case}: the crate's own sentence is the one the wire carries"
        );
    }

    // A read that asked for a SHARD is worded at the shard, in the sentence the CONDITION
    // earns: a stream this account and region do not hold at all states that the shard does
    // not exist, while a stream that IS held — whose shard map the id was looked up in and
    // missed — states that the shard could not be found in it.
    let shard_reads: Vec<(&str, &str, String, bool)> = vec![
        (
            "a shard of a stream nothing holds",
            absent,
            shard_id(0),
            false,
        ),
        ("another shard of that stream", absent, shard_id(7), false),
        (
            "a shard the held stream does not have",
            held,
            shard_id(9),
            true,
        ),
        (
            "a shard id of no shape this service mints",
            held,
            "shard-9".to_string(),
            true,
        ),
    ];
    for (case, stream, shard, stream_is_held) in shard_reads {
        let shard = shard.as_str();
        let (code, message) = refusal(
            &emulator,
            case,
            "GetShardIterator",
            &json!({
                "StreamName": stream,
                "ShardId": shard,
                "ShardIteratorType": SHARD_ITERATOR_TYPE_TRIM_HORIZON,
            }),
        )
        .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected the modelled not-found, got {code} with {message:?}"
        );
        let expected = if stream_is_held {
            format!(
                "Could not find shard {shard} in stream {stream} under account {}.",
                engine::EMULATOR_ACCOUNT_ID
            )
        } else {
            format!(
                "Shard {shard} in stream {stream} under account {} does not exist",
                engine::EMULATOR_ACCOUNT_ID
            )
        };
        assert_eq!(
            message, expected,
            "case {case}: expected the sentence that condition is worded with"
        );
        let crate_sentence = if stream_is_held {
            shard_not_in_stream_message(shard, stream)
        } else {
            shard_not_found_message(shard, stream)
        };
        assert_eq!(
            message, crate_sentence,
            "case {case}: the crate's own sentence is the one the wire carries"
        );
    }

    // A listing narrowed by a creation instant that is not this stream's names the instant
    // it was asked for, spelled out as the wire spells a timestamp and to whole seconds.
    let instants: Vec<(&str, f64, &str)> = vec![
        ("the epoch", 0.0, "1970-01-01T00:00:00Z"),
        ("a second past the epoch", 1.0, "1970-01-01T00:00:01Z"),
        (
            "an instant in 2026",
            1_793_406_179.0,
            "2026-10-31T00:22:59Z",
        ),
        // An instant carrying a finer fraction than a second — which is the precision a read
        // of a stream reports, so it is what a client sends back — is spelled to the second.
        (
            "an instant with milliseconds",
            1_793_406_179.588,
            "2026-10-31T00:22:59Z",
        ),
        (
            "an instant just under a second past the epoch",
            0.999,
            "1970-01-01T00:00:00Z",
        ),
    ];
    for (case, asked, spelled) in instants {
        for identifier in [json!({ "StreamName": held }), json!({ "StreamARN": arn })] {
            let mut body = identifier.clone();
            body["StreamCreationTimestamp"] = json!(asked);
            let (code, message) = refusal(&emulator, case, "ListShards", &body).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case}: expected the modelled not-found, got {code} with {message:?}"
            );
            assert_eq!(
                message,
                format!(
                    "Stream {held} under account {} with creation time {spelled} not found.",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "case {case}: expected the creation instant named in the refusal"
            );
            #[allow(clippy::cast_possible_truncation)]
            let secs = asked.floor() as i64;
            assert_eq!(
                message,
                stream_creation_time_not_found_message(held, secs),
                "case {case}: the crate's own sentence is the one the wire carries"
            );
        }
    }

    // The instant the stream really was created is SERVED, so the refusal above is about the
    // instant and not about the member being there at all.
    let created = emulator
        .ok(
            "the stream's own creation instant",
            "DescribeStreamSummary",
            &json!({ "StreamName": held }),
        )
        .await["StreamDescriptionSummary"]["StreamCreationTimestamp"]
        .clone();
    let listed = emulator
        .ok(
            "the stream's own creation instant",
            "ListShards",
            &json!({ "StreamName": held, "StreamCreationTimestamp": created }),
        )
        .await;
    assert_eq!(
        listed["Shards"].as_array().map(Vec::len),
        Some(2),
        "the stream's own creation instant narrows the listing to it, got {listed}"
    );
}

/// A `StreamName` and a `StreamARN` whose halves disagree are refused naming BOTH halves —
/// the name the request sent and the name its ARN carries — and the pair is reconciled from
/// the request, before any stream is looked up.
#[tokio::test]
async fn a_disagreeing_identifier_pair_names_both_halves() {
    let emulator = Emulator::new();
    let held = "pair-held";
    active_stream(&emulator, held, 1).await;
    let other = "pair-other";
    active_stream(&emulator, other, 1).await;

    // (case, the operation, the StreamName sent, the StreamARN sent)
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "both halves name a held stream",
            "ListShards",
            held,
            stream_arn(REGION, other),
        ),
        (
            "neither half names a held stream",
            "ListShards",
            "pair-absent",
            stream_arn(REGION, "pair-absent-too"),
        ),
        (
            "the ARN belongs to another region",
            "DescribeStream",
            held,
            stream_arn("us-west-2", other),
        ),
        (
            "a write naming two streams",
            "PutRecord",
            held,
            stream_arn(REGION, other),
        ),
        (
            "a read of a shard naming two streams",
            "GetShardIterator",
            held,
            stream_arn(REGION, other),
        ),
    ];

    for (case, operation, name, arn) in rows {
        let mut body = json!({ "StreamName": name, "StreamARN": arn });
        if operation == "PutRecord" {
            body["PartitionKey"] = json!("pk");
            body["Data"] = json!(aws_smithy_types::base64::encode("x"));
        }
        if operation == "GetShardIterator" {
            body["ShardId"] = json!(shard_id(0));
            body["ShardIteratorType"] = json!(SHARD_ITERATOR_TYPE_TRIM_HORIZON);
        }
        let (code, message) = refusal(&emulator, case, operation, &body).await;
        assert_eq!(
            code, "InvalidArgumentException",
            "case {case}: expected the modelled invalid-argument, got {code} with {message:?}"
        );
        let inferred = arn
            .rsplit_once("stream/")
            .map(|(_, name)| name)
            .expect("a stream ARN names its stream");
        assert_eq!(
            message,
            format!(
                "Input stream name {name} doesn't match the inferred name in stream arn \
                 {inferred}"
            ),
            "case {case}: expected both halves of the pair named"
        );
        assert_eq!(
            message,
            stream_identifier_mismatch_message(name, &arn),
            "case {case}: the crate's own sentence is the one the wire carries"
        );
    }

    // A pair that AGREES is served, which is the other half of the rule.
    let served = emulator
        .ok(
            "an agreeing pair",
            "ListShards",
            &json!({ "StreamName": held, "StreamARN": stream_arn(REGION, held) }),
        )
        .await;
    assert_eq!(
        served["Shards"].as_array().map(Vec::len),
        Some(1),
        "a pair whose halves agree names one stream and is served, got {served}"
    );
}

/// A create that names the on-demand capacity mode and a shard count is refused with the one
/// sentence that rule has, whatever count it named.
#[tokio::test]
async fn an_on_demand_create_naming_a_shard_count_is_refused() {
    let emulator = Emulator::new();
    for count in [1, 2, 7, 500] {
        let case = format!("on-demand with ShardCount {count}");
        let (code, message) = refusal(
            &emulator,
            &case,
            "CreateStream",
            &json!({
                "StreamName": format!("on-demand-{count}"),
                "ShardCount": count,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
        assert_eq!(
            code, "InvalidArgumentException",
            "case {case}: expected the modelled invalid-argument, got {code} with {message:?}"
        );
        assert_eq!(
            message, "ShardCount cannot be set while creating stream in On-Demand StreamMode",
            "case {case}: expected the fixed sentence for that rule"
        );
        assert_eq!(
            message, ON_DEMAND_SHARD_COUNT_MESSAGE,
            "case {case}: the crate's own sentence is the one the wire carries"
        );
        // Nothing was created: the refused name is still free.
        let (status, response) = emulator
            .call(
                "DescribeStreamSummary",
                &json!({ "StreamName": format!("on-demand-{count}") }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: a refused create stores nothing, got {status} with {response}"
        );
    }
}

/// An `ExplicitHashKey` past the 128-bit key space is refused as a RANGE violation of that
/// member, naming the range and the value as sent — while the last key INSIDE the space is
/// served.
#[tokio::test]
async fn an_explicit_hash_key_past_the_key_space_is_a_range_refusal() {
    let emulator = Emulator::new();
    let stream = "hash-key-range";
    active_stream(&emulator, stream, 1).await;

    // One past the space, further past it, and the largest value the member's own
    // `@pattern` admits (`^(0|([1-9]\d{0,38}))$`, 39 digits) — every one of them outside
    // `[0, 2^128-1]`.
    let outside = [
        PAST_THE_KEY_SPACE.to_string(),
        "340282366920938463463374607431768211500".to_string(),
        "9".repeat(39),
    ];
    for value in outside {
        let case = format!("an ExplicitHashKey of {value}");
        let (code, message) = refusal(
            &emulator,
            &case,
            "PutRecord",
            &json!({
                "StreamName": stream,
                "PartitionKey": "pk",
                "ExplicitHashKey": value.clone(),
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        )
        .await;
        assert_eq!(
            code, "InvalidArgumentException",
            "case {case}: expected the modelled invalid-argument, got {code} with {message:?}"
        );
        assert_eq!(
            message,
            format!(
                "Invalid ExplicitHashKey. ExplicitHashKey must be in the range: [0, 2^128-1]. \
                 Specified value was {value}"
            ),
            "case {case}: expected the range refusal naming the value"
        );
        assert_eq!(
            message,
            hash_key_out_of_range_message("ExplicitHashKey", &value),
            "case {case}: the crate's own sentence is the one the wire carries"
        );

        // The same rule inside a batch, where the refusal names the entry at fault as well.
        let (code, message) = refusal(
            &emulator,
            &case,
            "PutRecords",
            &json!({
                "StreamName": stream,
                "Records": [{
                    "PartitionKey": "pk",
                    "ExplicitHashKey": value.clone(),
                    "Data": aws_smithy_types::base64::encode("x"),
                }],
            }),
        )
        .await;
        assert_eq!(
            code, "InvalidArgumentException",
            "case {case}: expected the modelled invalid-argument in a batch, got {code}"
        );
        assert!(
            message.ends_with(&hash_key_out_of_range_message("ExplicitHashKey", &value)),
            "case {case}: expected the batch refusal to state the range violation, got \
             {message:?}"
        );
    }

    // The last key INSIDE the space is covered by the stream's one shard, so it is served:
    // the refusal above is about the range and not about the member being there at all.
    let written = emulator
        .ok(
            "the last key inside the space",
            "PutRecord",
            &json!({
                "StreamName": stream,
                "PartitionKey": "pk",
                "ExplicitHashKey": LAST_HASH_KEY,
                "Data": aws_smithy_types::base64::encode("x"),
            }),
        )
        .await;
    assert_eq!(
        written["ShardId"],
        json!(shard_id(0)),
        "the last key inside the key space lands in the shard that covers it, got {written}"
    );
}
