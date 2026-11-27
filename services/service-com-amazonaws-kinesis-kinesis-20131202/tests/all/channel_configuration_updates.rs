// SPDX-License-Identifier: CC0-1.0

//! A Kinesis CHANNEL's CONFIGURATION CHANGES, end to end over the wire: `UpdateChannel` ->
//! `DescribeChannel`.
//!
//! RULE: an accepted `UpdateChannel` writes exactly the members its request names onto the
//! STORED channel — the data-freshness window of the destination the channel already delivers
//! to, and the CloudWatch Logs settings, where a log group or log stream name the request
//! omits keeps the value the channel already carries — leaves every other member exactly as
//! it was, and takes the channel out of `ACTIVE` into `UPDATING` until the transition it armed
//! falls due. A channel that is not `ACTIVE` admits no further change: it is refused with the
//! modelled `ResourceInUseException` at HTTP 400 and nothing is written. So of any number of
//! overlapping changes exactly one is admitted, and the channel settles on exactly that one
//! caller's values rather than on a mixture of several callers'. A request that names nothing
//! to update, or that names the destination the channel does not deliver to ("You cannot
//! change the destination ... of an existing channel", `UpdateChannel`), is
//! `InvalidArgumentException`; one naming a channel this account and region do not hold is
//! `ResourceNotFoundException`; and neither arms any transition.
//!
//! The recorded flow this was derived from races TWO `UpdateChannel` calls carrying
//! `S3DestinationConfiguration.DataFreshnessInSeconds` against ONE channel, which is one row
//! of the tables below. The rows differ along what the rule generalises over: which updatable
//! member the request names (either destination's freshness window, the logging settings, or
//! both at once), the value it carries, whether the channel was already logging, which logging
//! names the request omits, the destination kind the channel delivers to, the lifecycle status
//! the channel is in when the call arrives, the region the request is signed for, and how many
//! callers overlap.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    channel_arn, channel_log_group_name, CHANNEL_UPDATE_DELAY,
};

use crate::channel_lifecycle::{
    dead_letter_queue, description, description_without_status, member, on_demand_stream,
    on_demand_stream_in, past_channel_activation, poll_until_channel_active_in, role_arn,
    schema_arn, storage_configuration, stream_configuration, table_bucket_arn,
};
use crate::stream_tags::{error_code, error_message, Emulator, REGION};

/// The status a channel reports while its create is still being applied: "CREATING - The
/// channel is being created" (`smithy_query(query="shape",shape="ChannelDescription")` ->
/// `ChannelStatus`).
const CREATING: &str = "CREATING";

/// The status a channel reports once it is ready to deliver records: "ACTIVE - The channel is
/// ready to deliver records" (same member).
const ACTIVE: &str = "ACTIVE";

/// The status a channel reports while an accepted change is being applied: "UPDATING - The
/// channel configuration is being updated" (same member).
const UPDATING: &str = "UPDATING";

/// The log stream a channel that names none logs to: "Defaults to DestinationDelivery"
/// (`CloudWatchLogs$LogStreamName`).
const DEFAULT_LOG_STREAM_NAME: &str = "DestinationDelivery";

/// A second region, so a case can show a change reaching the channels of the region the request
/// was signed for and no other.
const OTHER_REGION: &str = "eu-west-1";

/// How a refusal states that this account holds no channel under the identifier the request
/// named: the channel named the way a client addresses it, and the account serving the call.
fn not_found_message(channel_name: &str) -> String {
    format!(
        "Channel {channel_name} under account {} not found.",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// How long a case waits before treating an accepted change as applied: past the instant the
/// update armed the `UPDATING` -> `ACTIVE` transition for.
fn past_channel_update() -> Duration {
    CHANNEL_UPDATE_DELAY + Duration::from_millis(400)
}

/// A `LoggingConfiguration` body member, naming only the parts a case states.
fn logging(enabled: bool, log_group_name: Option<&str>, log_stream_name: Option<&str>) -> Value {
    let mut logs = json!({ "Enabled": enabled });
    if let Some(log_group_name) = log_group_name {
        logs["LogGroupName"] = json!(log_group_name);
    }
    if let Some(log_stream_name) = log_stream_name {
        logs["LogStreamName"] = json!(log_stream_name);
    }
    json!({ "CloudWatchLogs": logs })
}

/// A `CreateChannel` body for a channel named `case` delivering to a general purpose bucket.
fn s3_create(case: &str, source: &str, destination: Value, logging: Option<Value>) -> Value {
    let mut body = json!({
        "ChannelName": case,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(source, "JSON", None)],
        "S3DestinationConfiguration": destination,
    });
    if let Some(logging) = logging {
        body["LoggingConfiguration"] = logging;
    }
    body
}

/// A `CreateChannel` body for a channel named `case` delivering to streaming tables.
fn tables_create(case: &str, source: &str) -> Value {
    json!({
        "ChannelName": case,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(
            source,
            "GSR_JSON",
            Some(schema_arn("records", "iceberg")),
        )],
        "S3TablesDestinationConfiguration": {
            "DeadLetterQueueS3Configuration": dead_letter_queue("update-dlq", "tables/"),
            "S3TablesConfigurationList": [{
                "TableBucketARN": table_bucket_arn("update-tables"),
                "Namespace": "events",
                "TableName": "records",
                "CompressionType": "SNAPPY",
                "PartitionSpec": {
                    "PartitionFields": [{ "Transform": "TIME_HOUR", "SourceName": "written_at" }],
                },
            }],
        },
    })
}

/// One row of the update table: the channel `create` lays down, the `update` sent against it
/// (the driver adds the `ChannelARN`), and `patch`, which turns the description the create
/// answered into the one the channel must report afterwards. Whatever `patch` does not touch
/// must still read back exactly as the create left it.
struct Row {
    case: &'static str,
    create: Value,
    update: Value,
    patch: Box<dyn Fn(&mut Value)>,
}

/// The update table: one row per updatable member, value and starting logging state.
fn rows(source: &str) -> Vec<Row> {
    vec![
        Row {
            case: "freshness-to-900",
            create: s3_create(
                "freshness-to-900",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                None,
            ),
            update: json!({ "S3DestinationConfiguration": { "DataFreshnessInSeconds": 900 } }),
            patch: Box::new(|expected| {
                expected["S3DestinationConfiguration"]["DataFreshnessInSeconds"] = json!(900);
            }),
        },
        Row {
            case: "freshness-to-300",
            create: s3_create(
                "freshness-to-300",
                source,
                json!({
                    "DataFreshnessInSeconds": 600,
                    "StorageConfiguration": storage_configuration("update-bucket"),
                }),
                None,
            ),
            update: json!({ "S3DestinationConfiguration": { "DataFreshnessInSeconds": 300 } }),
            patch: Box::new(|expected| {
                expected["S3DestinationConfiguration"]["DataFreshnessInSeconds"] = json!(300);
            }),
        },
        Row {
            case: "freshness-to-450",
            create: s3_create(
                "freshness-to-450",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                None,
            ),
            update: json!({ "S3DestinationConfiguration": { "DataFreshnessInSeconds": 450 } }),
            patch: Box::new(|expected| {
                expected["S3DestinationConfiguration"]["DataFreshnessInSeconds"] = json!(450);
            }),
        },
        Row {
            case: "tables-freshness-to-400",
            create: tables_create("tables-freshness-to-400", source),
            update: json!({
                "S3TablesDestinationConfiguration": { "DataFreshnessInSeconds": 400 },
            }),
            patch: Box::new(|expected| {
                expected["S3TablesDestinationConfiguration"]["DataFreshnessInSeconds"] =
                    json!(400);
            }),
        },
        Row {
            case: "logging-on-named",
            create: s3_create(
                "logging-on-named",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                None,
            ),
            update: json!({
                "LoggingConfiguration": logging(true, Some("/channels/named"), Some("delivery")),
            }),
            patch: Box::new(|expected| {
                expected["LoggingConfiguration"] =
                    logging(true, Some("/channels/named"), Some("delivery"));
            }),
        },
        Row {
            case: "logging-on-defaults",
            create: s3_create(
                "logging-on-defaults",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                None,
            ),
            // A channel that starts logging and names neither takes the documented default
            // names, exactly as a create that names neither does.
            update: json!({ "LoggingConfiguration": logging(true, None, None) }),
            patch: Box::new(|expected| {
                let channel_id = expected["ChannelId"].as_str().unwrap_or_default().to_string();
                let log_group = channel_log_group_name("logging-on-defaults", &channel_id);
                expected["LoggingConfiguration"] =
                    logging(true, Some(&log_group), Some(DEFAULT_LOG_STREAM_NAME));
            }),
        },
        Row {
            case: "logging-off",
            create: s3_create(
                "logging-off",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                Some(logging(true, Some("/channels/off"), Some("delivery"))),
            ),
            update: json!({ "LoggingConfiguration": logging(false, None, None) }),
            patch: Box::new(|expected| {
                expected["LoggingConfiguration"]["CloudWatchLogs"]["Enabled"] = json!(false);
            }),
        },
        Row {
            case: "logging-stream-only",
            create: s3_create(
                "logging-stream-only",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                Some(logging(true, Some("/channels/keep"), Some("first"))),
            ),
            // The log group the update does not name keeps the value the channel carries.
            update: json!({ "LoggingConfiguration": logging(true, None, Some("second")) }),
            patch: Box::new(|expected| {
                expected["LoggingConfiguration"]["CloudWatchLogs"]["LogStreamName"] =
                    json!("second");
            }),
        },
        Row {
            case: "freshness-and-logging",
            create: s3_create(
                "freshness-and-logging",
                source,
                json!({ "StorageConfiguration": storage_configuration("update-bucket") }),
                None,
            ),
            update: json!({
                "S3DestinationConfiguration": { "DataFreshnessInSeconds": 700 },
                "LoggingConfiguration": logging(true, Some("/channels/both"), Some("both")),
            }),
            patch: Box::new(|expected| {
                expected["S3DestinationConfiguration"]["DataFreshnessInSeconds"] = json!(700);
                expected["LoggingConfiguration"] =
                    logging(true, Some("/channels/both"), Some("both"));
            }),
        },
    ]
}

/// An accepted change writes exactly the members it names, leaves every other one as it was,
/// reports `UPDATING` immediately and `ACTIVE` once the transition it armed falls due — and
/// `DescribeChannel` reports that same stored record throughout.
#[tokio::test]
async fn an_update_writes_exactly_the_members_it_names() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "update-source").await;
    let rows = rows(&source);

    // Every channel is laid down first, so ONE wait covers every create's transition.
    let mut created = Vec::with_capacity(rows.len());
    for row in &rows {
        created.push(emulator.ok(row.case, "CreateChannel", &row.create).await);
    }
    tokio::time::sleep(past_channel_activation()).await;

    let mut expected = Vec::with_capacity(rows.len());
    for (row, created) in rows.iter().zip(&created) {
        let case = row.case;
        let arn = channel_arn(REGION, case);
        let mut want = description_without_status(case, created);
        (row.patch)(&mut want);

        let mut body = row.update.clone();
        body["ChannelARN"] = json!(arn);
        let updated = emulator.ok(case, "UpdateChannel", &body).await;
        assert_eq!(
            description_without_status(case, &updated),
            want,
            "case {case}: expected the update to write exactly the members it names, got {}",
            description(case, &updated)
        );
        assert_eq!(
            member(case, &description(case, &updated), "ChannelStatus"),
            UPDATING,
            "case {case}: expected the accepted change to leave the channel {UPDATING}, got {}",
            description(case, &updated)
        );

        // The stored record is what a read reports, while the change is still being applied.
        let read = emulator
            .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
            .await;
        assert_eq!(
            description_without_status(case, &read),
            want,
            "case {case}: expected a read to report what the update wrote, got {}",
            description(case, &read)
        );
        assert_eq!(
            member(case, &description(case, &read), "ChannelStatus"),
            UPDATING,
            "case {case}: expected a read during the change to report {UPDATING}, got {}",
            description(case, &read)
        );
        expected.push(want);
    }

    // ONE wait covers every accepted change's transition back to `ACTIVE`.
    tokio::time::sleep(past_channel_update()).await;
    for (row, want) in rows.iter().zip(&expected) {
        let case = row.case;
        let read = emulator
            .ok(
                case,
                "DescribeChannel",
                &json!({ "ChannelARN": channel_arn(REGION, case) }),
            )
            .await;
        assert_eq!(
            member(case, &description(case, &read), "ChannelStatus"),
            ACTIVE,
            "case {case}: expected the channel to settle {ACTIVE}, got {}",
            description(case, &read)
        );
        assert_eq!(
            description_without_status(case, &read),
            *want,
            "case {case}: expected the settled channel to carry the values the update wrote, \
             got {}",
            description(case, &read)
        );
    }
}

/// Race `bodies` against one channel and assert exactly one caller wins: every loser answers
/// 400 `ResourceInUseException` naming the state in the way, and the winner's channel reports
/// `UPDATING`. Answers the raced member of the winner's description, as `reported` reads it.
async fn one_winner(
    emulator: &Emulator,
    case: &str,
    bodies: [Value; 4],
    reported: fn(&Value) -> Value,
) -> Value {
    let answers = tokio::join!(
        emulator.call("UpdateChannel", &bodies[0]),
        emulator.call("UpdateChannel", &bodies[1]),
        emulator.call("UpdateChannel", &bodies[2]),
        emulator.call("UpdateChannel", &bodies[3]),
    );
    let answers = [answers.0, answers.1, answers.2, answers.3];
    let winners: Vec<&(u16, Value)> = answers
        .iter()
        .filter(|(status, _)| *status == 200)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "case {case}: expected exactly one winner among {} overlapping changes, got \
         {answers:?}",
        answers.len()
    );
    for (status, response) in answers.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 400,
            "case {case}: expected a loser to answer 400, got {status} with body {response}"
        );
        assert_eq!(
            error_code(response),
            "ResourceInUseException",
            "case {case}: expected a loser to be refused with ResourceInUseException, got \
             {response}"
        );
        let says = format!(
            "Channel {case} under account {} not {ACTIVE}, instead in state {UPDATING}.",
            engine::EMULATOR_ACCOUNT_ID
        );
        assert_eq!(
            error_message(response),
            says,
            "case {case}: expected the refusal to state {says:?}, got {:?}",
            error_message(response)
        );
    }
    let winner = description(case, &winners[0].1);
    assert_eq!(
        member(case, &winner, "ChannelStatus"),
        UPDATING,
        "case {case}: expected the admitted change to leave the channel {UPDATING}, got \
         {winner}"
    );
    reported(&winner)
}

/// Of any number of overlapping changes exactly one is admitted, every other is refused with
/// `ResourceInUseException`, and the channel settles on the winner's value — never on a
/// mixture of the racers' values.
#[tokio::test]
async fn overlapping_changes_admit_exactly_one_winner() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "race-source").await;

    // Two channels, raced along two different members of a change: the destination's freshness
    // window, and the log stream the channel delivers its logs to.
    let freshness_case = "race-freshness";
    let logging_case = "race-logging";
    for case in [freshness_case, logging_case] {
        emulator
            .ok(
                case,
                "CreateChannel",
                &s3_create(
                    case,
                    &source,
                    json!({ "StorageConfiguration": storage_configuration("race-bucket") }),
                    None,
                ),
            )
            .await;
    }
    tokio::time::sleep(past_channel_activation()).await;

    let freshness_arn = channel_arn(REGION, freshness_case);
    let submitted_seconds: [i64; 4] = [300, 400, 700, 900];
    let freshness_bodies = submitted_seconds.map(|seconds| {
        json!({
            "ChannelARN": freshness_arn,
            "S3DestinationConfiguration": { "DataFreshnessInSeconds": seconds },
        })
    });
    let won = one_winner(
        &emulator,
        freshness_case,
        freshness_bodies,
        |description| description["S3DestinationConfiguration"]["DataFreshnessInSeconds"].clone(),
    )
    .await;
    let won_seconds = won.as_i64().unwrap_or_default();
    assert!(
        submitted_seconds.contains(&won_seconds),
        "case {freshness_case}: expected the admitted change to carry one of \
         {submitted_seconds:?}, got {won_seconds}"
    );

    let logging_arn = channel_arn(REGION, logging_case);
    let submitted_streams = ["racer-one", "racer-two", "racer-three", "racer-four"];
    let logging_bodies = submitted_streams.map(|stream| {
        json!({
            "ChannelARN": logging_arn,
            "LoggingConfiguration": logging(true, Some("/channels/race"), Some(stream)),
        })
    });
    let won = one_winner(
        &emulator,
        logging_case,
        logging_bodies,
        |description| description["LoggingConfiguration"]["CloudWatchLogs"]["LogStreamName"].clone(),
    )
    .await;
    let won_stream = won.as_str().unwrap_or_default().to_string();
    assert!(
        submitted_streams.contains(&won_stream.as_str()),
        "case {logging_case}: expected the admitted change to carry one of \
         {submitted_streams:?}, got {won_stream:?}"
    );

    tokio::time::sleep(past_channel_update()).await;

    let read = emulator
        .ok(
            freshness_case,
            "DescribeChannel",
            &json!({ "ChannelARN": freshness_arn }),
        )
        .await;
    let settled = description(freshness_case, &read);
    assert_eq!(
        member(freshness_case, &settled, "ChannelStatus"),
        ACTIVE,
        "case {freshness_case}: expected the channel to settle {ACTIVE}, got {settled}"
    );
    assert_eq!(
        settled["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
        Some(won_seconds),
        "case {freshness_case}: expected the channel to settle on the winner's \
         {won_seconds} seconds, got {settled}"
    );

    let read = emulator
        .ok(
            logging_case,
            "DescribeChannel",
            &json!({ "ChannelARN": logging_arn }),
        )
        .await;
    let settled = description(logging_case, &read);
    assert_eq!(
        member(logging_case, &settled, "ChannelStatus"),
        ACTIVE,
        "case {logging_case}: expected the channel to settle {ACTIVE}, got {settled}"
    );
    assert_eq!(
        settled["LoggingConfiguration"],
        logging(true, Some("/channels/race"), Some(&won_stream)),
        "case {logging_case}: expected the channel to settle on the winner's log stream \
         {won_stream:?}, got {settled}"
    );
}

/// A channel that is not `ACTIVE` admits no change: the call is refused with
/// `ResourceInUseException` naming the state in the way, writes nothing, and the channel takes
/// the next change once it is `ACTIVE` again.
#[tokio::test]
async fn a_channel_that_is_not_active_admits_no_change() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "status-gate-source").await;
    let case = "status-gate";
    let arn = channel_arn(REGION, case);
    emulator
        .ok(
            case,
            "CreateChannel",
            &s3_create(
                case,
                &source,
                json!({ "StorageConfiguration": storage_configuration("status-gate-bucket") }),
                None,
            ),
        )
        .await;

    let first = json!({
        "ChannelARN": arn,
        "S3DestinationConfiguration": { "DataFreshnessInSeconds": 450 },
    });
    let second = json!({
        "ChannelARN": arn,
        "S3DestinationConfiguration": { "DataFreshnessInSeconds": 800 },
    });

    // CREATING: the create's own transition is still due, so the channel is not open to a
    // change yet.
    let (status, response) = emulator.call("UpdateChannel", &first).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a change to a {CREATING} channel to be refused, got {status} \
         with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {case}: expected ResourceInUseException, got {response}"
    );
    let says = format!(
        "Channel {case} under account {} not {ACTIVE}, instead in state {CREATING}.",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert_eq!(
        error_message(&response),
        says,
        "case {case}: expected the refusal to state {says:?}, got {:?}",
        error_message(&response)
    );

    tokio::time::sleep(past_channel_activation()).await;
    let accepted = emulator.ok(case, "UpdateChannel", &first).await;
    assert_eq!(
        member(case, &description(case, &accepted), "ChannelStatus"),
        UPDATING,
        "case {case}: expected the accepted change to leave the channel {UPDATING}, got {}",
        description(case, &accepted)
    );

    // UPDATING: the change just accepted is still being applied, so the next one is refused
    // and writes nothing.
    let (status, response) = emulator.call("UpdateChannel", &second).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a change to an {UPDATING} channel to be refused, got {status} \
         with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {case}: expected ResourceInUseException, got {response}"
    );
    let says = format!(
        "Channel {case} under account {} not {ACTIVE}, instead in state {UPDATING}.",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert_eq!(
        error_message(&response),
        says,
        "case {case}: expected the refusal to state {says:?}, got {:?}",
        error_message(&response)
    );

    let read = emulator
        .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
        .await;
    let during = description(case, &read);
    assert_eq!(
        during["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
        Some(450),
        "case {case}: expected the refused change to have written nothing, got {during}"
    );

    tokio::time::sleep(past_channel_update()).await;
    let read = emulator
        .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
        .await;
    let settled = description(case, &read);
    assert_eq!(
        member(case, &settled, "ChannelStatus"),
        ACTIVE,
        "case {case}: expected the channel to settle {ACTIVE}, got {settled}"
    );
    assert_eq!(
        settled["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
        Some(450),
        "case {case}: expected the admitted change to be the one that settled, got {settled}"
    );

    // ACTIVE again: the channel takes the next change.
    let accepted = emulator.ok(case, "UpdateChannel", &second).await;
    let reported = description(case, &accepted);
    assert_eq!(
        reported["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
        Some(800),
        "case {case}: expected the next change to be admitted once the channel is {ACTIVE}, \
         got {reported}"
    );
}

/// One row of the refusal table: the `update` sent against the channel `arn` names, the error
/// it earns and the text that error states.
struct RefusalRow {
    case: &'static str,
    arn: String,
    update: Value,
    code: &'static str,
    says: String,
}

/// A change this operation cannot make is refused with the modelled error, writes nothing and
/// arms no transition — the channel is still `ACTIVE` right afterwards.
#[tokio::test]
async fn a_change_the_operation_cannot_make_is_refused_and_arms_nothing() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "refusal-source").await;
    let bucket_channel = "refusal-bucket-channel";
    let tables_channel = "refusal-tables-channel";
    emulator
        .ok(
            bucket_channel,
            "CreateChannel",
            &s3_create(
                bucket_channel,
                &source,
                json!({ "StorageConfiguration": storage_configuration("refusal-bucket") }),
                None,
            ),
        )
        .await;
    emulator
        .ok(
            tables_channel,
            "CreateChannel",
            &tables_create(tables_channel, &source),
        )
        .await;
    tokio::time::sleep(past_channel_activation()).await;

    let bucket_arn_value = channel_arn(REGION, bucket_channel);
    let tables_arn_value = channel_arn(REGION, tables_channel);
    let missing = "refusal-no-such-channel";
    let rows = vec![
        RefusalRow {
            case: "names-nothing-to-update",
            arn: bucket_arn_value.clone(),
            update: json!({}),
            code: "InvalidArgumentException",
            says: "Specify at least one of S3DestinationConfiguration".to_string(),
        },
        RefusalRow {
            case: "streaming-tables-update-on-a-bucket-channel",
            arn: bucket_arn_value.clone(),
            update: json!({
                "S3TablesDestinationConfiguration": { "DataFreshnessInSeconds": 400 },
            }),
            code: "InvalidArgumentException",
            says: format!(
                "channel {bucket_arn_value} delivers to the S3 destination; you cannot change \
                 the destination"
            ),
        },
        RefusalRow {
            case: "bucket-update-on-a-streaming-tables-channel",
            arn: tables_arn_value.clone(),
            update: json!({ "S3DestinationConfiguration": { "DataFreshnessInSeconds": 400 } }),
            code: "InvalidArgumentException",
            says: format!(
                "channel {tables_arn_value} delivers to the S3_TABLES destination; you cannot \
                 change the destination"
            ),
        },
        RefusalRow {
            case: "no-such-channel",
            arn: channel_arn(REGION, missing),
            update: json!({ "S3DestinationConfiguration": { "DataFreshnessInSeconds": 400 } }),
            code: "ResourceNotFoundException",
            says: not_found_message(missing),
        },
    ];

    // What both held channels report before any refusal, which is what they must still report
    // after every one of them.
    let held: Vec<(String, Value)> = {
        let mut held = Vec::new();
        for (case, arn) in [
            (bucket_channel, &bucket_arn_value),
            (tables_channel, &tables_arn_value),
        ] {
            let read = emulator
                .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
                .await;
            held.push((arn.clone(), description(case, &read)));
        }
        held
    };

    for row in &rows {
        let case = row.case;
        let mut body = row.update.clone();
        body["ChannelARN"] = json!(row.arn);
        let (status, response) = emulator.call("UpdateChannel", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {case}: expected {}, got {response}",
            row.code
        );
        assert!(
            error_message(&response).contains(&row.says),
            "case {case}: expected the refusal to name {:?}, got {:?}",
            row.says,
            error_message(&response)
        );

        // A refused change leaves both channels exactly as they were, `ACTIVE` included: it
        // armed no transition.
        for (arn, before) in &held {
            let read = emulator
                .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
                .await;
            assert_eq!(
                description(case, &read),
                *before,
                "case {case}: expected the refused change to leave the channel as it was, got \
                 {}",
                description(case, &read)
            );
            assert_eq!(
                member(case, &description(case, &read), "ChannelStatus"),
                ACTIVE,
                "case {case}: expected the refused change to arm no transition, got {}",
                description(case, &read)
            );
        }
    }
}

/// A change reaches the channels of the region the request was signed for and NO other: a
/// channel's "name is unique within your Amazon Web Services account and Amazon Web Services
/// Region" (`CreateChannelInput$ChannelName`), so one name in two regions is two channels, each
/// taking its own changes, and a change signed for one region naming the other's channel
/// addresses a channel this endpoint holds none of — it is refused `ResourceNotFoundException`,
/// writes nothing and arms nothing.
#[tokio::test]
async fn a_change_reaches_only_the_channels_of_the_region_it_was_signed_for() {
    let emulator = Emulator::new();
    let case = "region-scoped";

    // ONE channel name in two regions, each starting on its own freshness window — so a change
    // that reached the wrong region's record would be visible as that region's value moving.
    let created: [(&str, i64); 2] = [(REGION, 320), (OTHER_REGION, 480)];
    for (region, seconds) in created {
        let source = on_demand_stream_in(&emulator, region, "region-scope-source").await;
        emulator
            .ok_in(
                case,
                region,
                "CreateChannel",
                &s3_create(
                    case,
                    &source,
                    json!({
                        "DataFreshnessInSeconds": seconds,
                        "StorageConfiguration": storage_configuration("region-scope-bucket"),
                    }),
                    None,
                ),
            )
            .await;
    }
    // Each channel is waited for by LOOKING at it until it reports the `ACTIVE` its create owes
    // it, so a loaded host only makes the case look again.
    for (region, _) in created {
        poll_until_channel_active_in(&emulator, case, region, &channel_arn(region, case)).await;
    }

    // A change signed for one region naming the OTHER region's channel, in both directions.
    for (region, elsewhere) in [(REGION, OTHER_REGION), (OTHER_REGION, REGION)] {
        let body = json!({
            "ChannelARN": channel_arn(elsewhere, case),
            "S3DestinationConfiguration": { "DataFreshnessInSeconds": 777 },
        });
        let (status, response) = emulator.call_in(region, "UpdateChannel", &body).await;
        assert_eq!(
            status, 400,
            "case {case} signed for {region}: expected a change naming {elsewhere}'s channel to \
             be refused, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case} signed for {region}: expected ResourceNotFoundException, got {response}"
        );
        let says = not_found_message(case);
        assert!(
            error_message(&response).contains(&says),
            "case {case} signed for {region}: expected the refusal to name {says:?}, got {:?}",
            error_message(&response)
        );
    }

    // Neither refusal wrote anything and neither armed a transition: both channels still carry
    // the window their create set, and both are still `ACTIVE`.
    for (region, seconds) in created {
        let held = describe_channel_in(&emulator, case, region).await;
        assert_eq!(
            member(case, &held, "ChannelStatus"),
            ACTIVE,
            "case {case} in {region}: expected the refused change to arm no transition, got {held}"
        );
        assert_eq!(
            held["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
            Some(seconds),
            "case {case} in {region}: expected the refused change to have written nothing, got \
             {held}"
        );
    }

    // Each region's channel takes its OWN change, and the other region's record does not move
    // with it: the change is applied where it was signed for and nowhere else.
    let changed: [(&str, i64); 2] = [(REGION, 600), (OTHER_REGION, 900)];
    for (index, (region, seconds)) in changed.iter().enumerate() {
        // What the region this change is NOT signed for holds right before it, which is what
        // that region must still hold right after it.
        let elsewhere = created[1 - index].0;
        let before = describe_channel_in(&emulator, case, elsewhere).await;
        let accepted = emulator
            .ok_in(
                case,
                region,
                "UpdateChannel",
                &json!({
                    "ChannelARN": channel_arn(region, case),
                    "S3DestinationConfiguration": { "DataFreshnessInSeconds": seconds },
                }),
            )
            .await;
        let reported = description(case, &accepted);
        assert_eq!(
            member(case, &reported, "ChannelStatus"),
            UPDATING,
            "case {case} in {region}: expected the accepted change to leave the channel \
             {UPDATING}, got {reported}"
        );
        assert_eq!(
            reported["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
            Some(*seconds),
            "case {case} in {region}: expected the accepted change to carry {seconds}, got \
             {reported}"
        );

        // The region this change was NOT signed for still reports the configuration it held:
        // the change reached the channels of the region it was signed for and no other.
        let held = describe_channel_in(&emulator, case, elsewhere).await;
        assert_eq!(
            held["S3DestinationConfiguration"],
            before["S3DestinationConfiguration"],
            "case {case}: expected a change signed for {region} to leave {elsewhere}'s channel \
             as it was, got {held}"
        );
    }

    // Each region's channel is looked at until it reports the `ACTIVE` its accepted change owes
    // it, and each settles on the value ITS caller submitted.
    for (region, _) in changed {
        poll_until_channel_active_in(&emulator, case, region, &channel_arn(region, case)).await;
    }
    for (region, seconds) in changed {
        let settled = describe_channel_in(&emulator, case, region).await;
        assert_eq!(
            member(case, &settled, "ChannelStatus"),
            ACTIVE,
            "case {case} in {region}: expected the channel to settle {ACTIVE}, got {settled}"
        );
        assert_eq!(
            settled["S3DestinationConfiguration"]["DataFreshnessInSeconds"].as_i64(),
            Some(seconds),
            "case {case} in {region}: expected the channel to settle on {seconds}, got {settled}"
        );
    }
}

/// The `ChannelDescription` `region`'s endpoint reports for the channel named `case`.
async fn describe_channel_in(emulator: &Emulator, case: &str, region: &str) -> Value {
    let read = emulator
        .ok_in(
            case,
            region,
            "DescribeChannel",
            &json!({ "ChannelARN": channel_arn(region, case) }),
        )
        .await;
    description(case, &read)
}
