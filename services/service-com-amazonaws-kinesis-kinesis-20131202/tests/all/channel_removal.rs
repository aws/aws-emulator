// SPDX-License-Identifier: CC0-1.0

//! Deleting a Kinesis CHANNEL, end to end over the wire: `CreateChannel` -> `DeleteChannel`
//! -> `DescribeChannel` / `ListChannels`.
//!
//! RULE: `DeleteChannel` REMOVES the channel this account holds under the `ChannelARN` the
//! request names, inside the region the request was signed for — "Deleting a channel stops
//! delivery from the source stream to the destination" — so from the answer on, the channel is
//! no resource of this account: `DescribeChannel` and `ListChannels` report it gone, its name
//! is free for another create, and nothing else the account holds moves (the source stream it
//! read from included, since "Data already delivered to the destination is not deleted"). The
//! delete is served whatever lifecycle status the channel is in — the model declares this
//! operation no "the resource must be in the ACTIVE state" error at all, its errors being
//! `AccessDeniedException`, `InvalidArgumentException`, `LimitExceededException`,
//! `ResourceNotFoundException` and `ValidationException`
//! (`smithy_query(query="operation",operation="DeleteChannel")`) — and an identifier this
//! account and region hold no channel under is refused `ResourceNotFoundException`, which is
//! also what a second delete of one channel earns.
//!
//! The recorded flow this was derived from creates ONE channel delivering to one general
//! purpose bucket, waits for it to become `ACTIVE` and deletes it, which is one row of the
//! tables below. The rows differ along what the rule generalises over: the destination kind,
//! the record format, which optional members the create named, the region the request is
//! signed for, how many channels the region holds when the delete arrives, and the lifecycle
//! status the channel is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    channel_arn, CHANNEL_ACTIVE, CHANNEL_CREATING, CHANNEL_UPDATING, STREAM_MODE_ON_DEMAND,
};

// The one wire harness this crate's suites share, and the channel-building helpers the create
// suite already carries — this suite adds the deletion steps rather than a second copy of them.
use crate::channel_lifecycle::{
    dead_letter_queue, description, member, past_channel_activation, poll_until_channel_active,
    role_arn, schema_arn, storage_configuration, stream_configuration, table_bucket_arn,
};
use crate::stream_tags::{
    error_code, error_message, past_activation, stream_arn, Emulator, REGION,
};

/// A second region, so a case can show that a delete reaches the channels of the region the
/// request was signed for and no other.
const OTHER_REGION: &str = "eu-west-1";

/// How a refusal states that this account holds no channel under the identifier the request
/// named: the wording the service answers for a channel it cannot find, naming the channel by
/// name and the account serving the call.
fn not_found_message(channel_name: &str) -> String {
    format!(
        "Channel {channel_name} under account {} not found.",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Call `operation` in `region` expecting a client refusal, answering its error code and
/// message.
async fn refused(
    emulator: &Emulator,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> (String, String) {
    let (status, response) = emulator.call_in(region, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case}: expected {operation} to answer 400, got {status} with body {response}"
    );
    (error_code(&response), error_message(&response))
}

/// The `ChannelStatus` a `DescribeChannel` reports for the channel `arn` names, or the error
/// code the read was refused with.
async fn channel_status(emulator: &Emulator, region: &str, arn: &str) -> Result<String, String> {
    let (status, response) = emulator
        .call_in(region, "DescribeChannel", &json!({ "ChannelARN": arn }))
        .await;
    if status == 200 {
        Ok(member("status", &description("status", &response), "ChannelStatus"))
    } else {
        Err(error_code(&response))
    }
}

/// The channel names `ListChannels` reports for `region`, in the order it reported them.
async fn listed_names(emulator: &Emulator, case: &str, region: &str) -> Vec<String> {
    let listed = emulator
        .ok_in(case, region, "ListChannels", &json!({}))
        .await;
    listed
        .get("ChannelSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: ListChannels must carry ChannelSummaries, got {listed}"))
        .iter()
        .map(|summary| member(case, summary, "ChannelName"))
        .collect()
}

/// An `ACTIVE` on-demand stream named `name` in each of `regions` — the source every channel
/// case reads from. One create per region and ONE wait for all of them.
async fn on_demand_source_in(emulator: &Emulator, regions: &[&str], name: &str) {
    for region in regions {
        emulator
            .ok_in(
                name,
                region,
                "CreateStream",
                &json!({
                    "StreamName": name,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
                }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;
}

/// One row of the deletion table: the channel `body` creates in `region`, whose removal must
/// leave the region holding every channel but it.
struct Row {
    case: &'static str,
    region: &'static str,
    body: Value,
}

/// The rows every deletion case runs: the destination kind, the record format inside the
/// catalog that destination admits, which optional members the create names, and the region the
/// request is signed for all differ between them.
fn rows(source_here: &str, source_there: &str) -> Vec<Row> {
    vec![
        Row {
            case: "s3-minimal",
            region: REGION,
            body: json!({
                "ChannelName": "s3-minimal",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(source_here, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("removal-bucket-one"),
                },
            }),
        },
        Row {
            case: "s3-logged-tagged-encrypted",
            region: REGION,
            body: json!({
                "ChannelName": "s3-logged-tagged-encrypted",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(source_here, "STRING", None)],
                "S3DestinationConfiguration": {
                    "DataFreshnessInSeconds": 60,
                    "DeadLetterQueueS3Configuration": dead_letter_queue("removal-dlq", "failed/"),
                    "StorageConfiguration": storage_configuration("removal-bucket-two"),
                },
                "LoggingConfiguration": { "CloudWatchLogs": { "Enabled": true } },
                "EncryptionConfiguration": {
                    "EncryptionType": "KMS",
                    "KeyId": "alias/removal-key",
                },
                "Tags": { "team": "data" },
            }),
        },
        Row {
            case: "tables-gsr-json",
            region: REGION,
            body: json!({
                "ChannelName": "tables-gsr-json",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(
                    source_here,
                    "GSR_JSON",
                    Some(schema_arn("records", "iceberg")),
                )],
                "S3TablesDestinationConfiguration": {
                    "DeadLetterQueueS3Configuration": dead_letter_queue("removal-dlq", "tables/"),
                    "S3TablesConfigurationList": [{
                        "TableBucketARN": table_bucket_arn("removal-tables"),
                        "Namespace": "events",
                        "TableName": "records",
                        "CompressionType": "SNAPPY",
                    }],
                },
            }),
        },
        Row {
            case: "s3-in-another-region",
            region: OTHER_REGION,
            body: json!({
                "ChannelName": "s3-in-another-region",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(source_there, "BYTE_ARRAY", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("removal-bucket-three"),
                },
            }),
        },
    ]
}

/// A deleted channel is gone for every later read, the channels beside it stand, the source
/// stream it read from stands, and its name is free for another create.
#[tokio::test]
async fn a_deleted_channel_is_gone_and_its_name_is_free() {
    let emulator = Emulator::new();
    let source = "removal-source";
    on_demand_source_in(&emulator, &[REGION, OTHER_REGION], source).await;
    let rows = rows(&stream_arn(REGION, source), &stream_arn(OTHER_REGION, source));

    // Every channel the table names is created first and waited for ONCE, so each delete
    // arrives at an `ACTIVE` channel in a region that holds more than it.
    for row in &rows {
        emulator
            .ok_in(row.case, row.region, "CreateChannel", &row.body)
            .await;
    }
    tokio::time::sleep(past_channel_activation()).await;

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let arn = channel_arn(row.region, case);
        let held_before = listed_names(&emulator, case, row.region).await;
        assert!(
            held_before.contains(&case.to_string()),
            "case {case}: expected {row_region} to list the channel before the delete, got \
             {held_before:?}",
            row_region = row.region
        );
        let id_before = member(
            case,
            &description(
                case,
                &emulator
                    .ok_in(case, row.region, "DescribeChannel", &json!({ "ChannelARN": arn }))
                    .await,
            ),
            "ChannelId",
        );

        let answer = emulator
            .ok_in(case, row.region, "DeleteChannel", &json!({ "ChannelARN": arn }))
            .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: DeleteChannel models no output members, so it answers an empty body, \
             got {answer}"
        );

        // From the answer on the channel is no resource of this account: neither read reports
        // it, and a second delete of it is refused the way a delete of a channel nothing
        // created is.
        assert_eq!(
            channel_status(&emulator, row.region, &arn).await,
            Err("ResourceNotFoundException".to_string()),
            "case {case}: expected the deleted channel to be read as one this account does not \
             hold"
        );
        let listed = listed_names(&emulator, case, row.region).await;
        assert!(
            !listed.contains(&case.to_string()),
            "case {case}: expected the deleted channel to be listed no more, got {listed:?}"
        );
        let (code, message) =
            refused(&emulator, case, row.region, "DeleteChannel", &json!({ "ChannelARN": arn }))
                .await;
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("ResourceNotFoundException", not_found_message(case).as_str()),
            "case {case}: expected a second delete to be refused as a channel this account does \
             not hold, got {code} with message {message:?}"
        );

        // Every channel the delete did not name still stands, in its own region.
        for other in rows.iter().skip(index + 1) {
            assert_eq!(
                channel_status(&emulator, other.region, &channel_arn(other.region, other.case))
                    .await,
                Ok(CHANNEL_ACTIVE.to_string()),
                "case {case}: expected deleting it to leave {other_case} exactly as it was",
                other_case = other.case
            );
        }

        // "Data already delivered to the destination is not deleted": the source stream the
        // channel read from is untouched by the channel's removal.
        emulator
            .ok_in(
                case,
                row.region,
                "DescribeStreamSummary",
                &json!({ "StreamName": source }),
            )
            .await;

        // The name is free again, and what a create mints under it is ANOTHER channel: its own
        // id, reported as `CREATING` from the start.
        let recreated = description(
            case,
            &emulator
                .ok_in(case, row.region, "CreateChannel", &row.body)
                .await,
        );
        assert_eq!(
            member(case, &recreated, "ChannelStatus"),
            CHANNEL_CREATING,
            "case {case}: expected the re-created channel to start CREATING, got {recreated}"
        );
        assert_ne!(
            member(case, &recreated, "ChannelId"),
            id_before,
            "case {case}: expected the channel re-created under a freed name to be another \
             channel with its own id, got {recreated}"
        );
    }
}

/// A channel still inside a transition is deleted like any other: the model declares this
/// operation no not-ACTIVE refusal, so the record goes and the identifier answers nothing
/// from then on.
#[tokio::test]
async fn a_channel_inside_a_transition_is_deleted_too() {
    let emulator = Emulator::new();
    let source = "transition-source";
    on_demand_source_in(&emulator, &[REGION], source).await;
    let source_arn = stream_arn(REGION, source);

    // (the case, the status the channel is in when the delete arrives, whether an accepted
    // update is what put it there)
    let cases: [(&str, &str, bool); 2] = [
        ("creating", CHANNEL_CREATING, false),
        ("updating", CHANNEL_UPDATING, true),
    ];

    for (case, transient, through_update) in cases {
        let body = json!({
            "ChannelName": case,
            "ServiceExecutionRoleARN": role_arn("channel-delivery"),
            "StreamConfigurationList": [stream_configuration(&source_arn, "JSON", None)],
            "S3DestinationConfiguration": {
                "StorageConfiguration": storage_configuration("transition-bucket"),
            },
        });
        emulator.ok(case, "CreateChannel", &body).await;
        let arn = channel_arn(REGION, case);
        if through_update {
            poll_until_channel_active(&emulator, case, &arn).await;
            emulator
                .ok(
                    case,
                    "UpdateChannel",
                    &json!({
                        "ChannelARN": arn,
                        "S3DestinationConfiguration": { "DataFreshnessInSeconds": 120 },
                    }),
                )
                .await;
        }

        assert_eq!(
            channel_status(&emulator, REGION, &arn).await,
            Ok(transient.to_string()),
            "case {case}: expected the channel to be {transient} when the delete arrives"
        );
        emulator
            .ok(case, "DeleteChannel", &json!({ "ChannelARN": arn }))
            .await;
        assert_eq!(
            channel_status(&emulator, REGION, &arn).await,
            Err("ResourceNotFoundException".to_string()),
            "case {case}: expected the delete of a {transient} channel to remove it"
        );
    }
}

/// A delete naming a channel this account and region hold none of is refused, and removes
/// nothing: the region the request was signed for is the only one whose channels it reaches.
#[tokio::test]
async fn a_delete_of_a_channel_this_region_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let source = "absent-source";
    on_demand_source_in(&emulator, &[REGION, OTHER_REGION], source).await;

    // One channel of each region, under ONE name, so a case can show that a delete signed for
    // a region reaches that region's channel and never the other's.
    let held = "held-channel";
    for region in [REGION, OTHER_REGION] {
        emulator
            .ok_in(
                held,
                region,
                "CreateChannel",
                &json!({
                    "ChannelName": held,
                    "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                    "StreamConfigurationList": [stream_configuration(
                        &stream_arn(region, source),
                        "JSON",
                        None,
                    )],
                    "S3DestinationConfiguration": {
                        "StorageConfiguration": storage_configuration("absent-bucket"),
                    },
                }),
            )
            .await;
    }
    tokio::time::sleep(past_channel_activation()).await;

    // (the case, the region the delete is signed for, the ARN it names, the channel name the
    // refusal must name). The rows differ along why no channel answers to the identifier: a
    // name nothing created, a name held only in ANOTHER region, an ARN of another region's
    // endpoint, and a name differing from a held one by one character.
    let rows: [(&str, &str, String, &str); 4] = [
        (
            "never-created",
            REGION,
            channel_arn(REGION, "no-such-channel"),
            "no-such-channel",
        ),
        (
            "held-only-elsewhere",
            REGION,
            channel_arn(OTHER_REGION, held),
            held,
        ),
        (
            "held-only-here",
            OTHER_REGION,
            channel_arn(REGION, held),
            held,
        ),
        (
            "one-character-off",
            REGION,
            channel_arn(REGION, "held_channel"),
            "held_channel",
        ),
    ];

    for (case, region, arn, named) in &rows {
        let (code, message) =
            refused(&emulator, case, region, "DeleteChannel", &json!({ "ChannelARN": arn })).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case}: expected a channel this account and region hold none of to be refused \
             as not found, got {code} with message {message:?}"
        );
        assert_eq!(
            message,
            not_found_message(named),
            "case {case}: expected the refusal to name the channel and the account serving the \
             call, got {message:?}"
        );
    }

    // Nothing was removed: both regions still hold their channel.
    for region in [REGION, OTHER_REGION] {
        assert_eq!(
            channel_status(&emulator, region, &channel_arn(region, held)).await,
            Ok(CHANNEL_ACTIVE.to_string()),
            "case {region}: expected every refused delete to leave the held channel as it was"
        );
    }
}

/// Of any number of overlapping deletes of ONE channel, exactly one removes it: the delete
/// takes the store's lock for the whole read-and-remove, so every other one finds no channel
/// under the ARN.
#[tokio::test]
async fn overlapping_deletes_of_one_channel_admit_exactly_one() {
    let emulator = Emulator::new();
    let source = "race-source";
    on_demand_source_in(&emulator, &[REGION], source).await;
    let case = "race-channel";
    emulator
        .ok(
            case,
            "CreateChannel",
            &json!({
                "ChannelName": case,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(
                    &stream_arn(REGION, source),
                    "JSON",
                    None,
                )],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("race-bucket"),
                },
            }),
        )
        .await;
    tokio::time::sleep(past_channel_activation()).await;

    let body = json!({ "ChannelARN": channel_arn(REGION, case) });
    let answers = tokio::join!(
        emulator.call("DeleteChannel", &body),
        emulator.call("DeleteChannel", &body),
        emulator.call("DeleteChannel", &body),
    );
    let answers = [answers.0, answers.1, answers.2];
    let accepted = answers.iter().filter(|(status, _)| *status == 200).count();
    assert_eq!(
        accepted, 1,
        "case {case}: expected exactly one winner among {} overlapping deletes, got {answers:?}",
        answers.len()
    );
    for (status, response) in answers.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 400,
            "case {case}: expected the losers to answer 400, got {status} with body {response}"
        );
        assert_eq!(
            error_code(response),
            "ResourceNotFoundException",
            "case {case}: expected the loser to be told the channel is gone, got {response}"
        );
        assert_eq!(
            error_message(response),
            not_found_message(case),
            "case {case}: expected the loser's refusal to name the channel and the account, got \
             {response}"
        );
    }
}
