// SPDX-License-Identifier: CC0-1.0

//! A Kinesis CHANNEL, end to end over the wire: `CreateChannel` -> `DescribeChannel`.
//!
//! RULE: `CreateChannel` PERSISTS a channel under the `ChannelName` its request carries,
//! scoped to the region that request was signed for, and answers the stored record projected
//! onto `ChannelDescription`: the ARN and the id the channel is addressed by, the creation
//! instant, the service execution role, the source streams it reads (each carrying the STORED
//! stream's own creation instant), the destination it delivers to with every documented
//! default resolved (the 300-second data-freshness window, the `STANDARD` storage class, a
//! dead-letter queue defaulting to the destination bucket under an error prefix), the
//! CloudWatch Logs configuration with its documented default names, and the encryption
//! configuration the request named. It reports `CREATING` until the transition the create
//! armed falls due and `ACTIVE` from then on, and `DescribeChannel` projects that same STORED
//! record, so what a create accepted is what a read reports and a member the create did not
//! name stays absent. A name the region already holds a channel under is refused with the
//! modelled `ResourceInUseException` and the stored channel is left untouched, while the same
//! name in another region is another channel. A request that names no destination or both is
//! `InvalidArgumentException`, as is one naming more source streams than a channel carries or
//! a record format its destination does not admit; a source stream this account and region do
//! not hold is `ResourceNotFoundException`, one that is not `ACTIVE` is
//! `ResourceInUseException`, and a source stream whose capacity the caller provisions or a
//! KMS key that is no customer managed key is `ValidationException`. A read naming a channel
//! this account and region do not hold is `ResourceNotFoundException`.
//!
//! The recorded flow this was derived from creates ONE channel delivering to one general
//! purpose bucket, which is one row of the tables below. The rows differ along what the rule
//! generalises over: the destination kind, the record format inside the catalog each
//! destination admits, which optional members the create names, the KMS key identifier form,
//! the region the request is signed for, and the lifecycle status the source stream is in when
//! the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    channel_arn, CHANNEL_ACTIVATION_DELAY, CHANNEL_ACTIVE, MAX_CHANNELS_PER_REGION,
    STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::stream_tags::{error_code, error_message, stream_arn, Emulator, REGION};

/// The status a freshly created channel reports: "CREATING - The channel is being created"
/// (`smithy_query(query="shape",shape="ChannelSummary")` -> `ChannelStatus`).
const CREATING: &str = "CREATING";

/// The status a provisioned channel reports: "ACTIVE - The channel is ready to deliver
/// records" (same member).
const ACTIVE: &str = "ACTIVE";

/// The data-freshness window a destination that names none gets: "The default value is 300
/// seconds" (`S3DestinationConfiguration$DataFreshnessInSeconds`).
const DEFAULT_FRESHNESS_SECONDS: i64 = 300;

/// The storage class delivered objects get when the create names none: "STANDARD - The
/// default storage class" (`S3StorageConfiguration$StorageClass`).
const DEFAULT_STORAGE_CLASS: &str = "STANDARD";

/// The log stream a channel that names none logs to: "Defaults to DestinationDelivery"
/// (`CloudWatchLogs$LogStreamName`).
const DEFAULT_LOG_STREAM_NAME: &str = "DestinationDelivery";

/// The key prefix the dead-letter queue a general purpose destination gets by default
/// writes under: that queue "defaults to the destination bucket with an error prefix"
/// (`S3DestinationConfiguration$DeadLetterQueueS3Configuration`), whose text the emulator
/// chooses — so this is the value it must keep.
const DEFAULT_ERROR_PREFIX: &str = "errors/";

/// The alias of the KMS key this service owns, which no channel may be encrypted with:
/// "Master key owned by Kinesis Data Streams: alias/aws/kinesis"
/// (`StartStreamEncryptionInput$KeyId`).
const SERVICE_KEY_ALIAS: &str = "alias/aws/kinesis";

/// How long a case waits before treating a freshly created channel as ready: past the instant
/// the create armed the `CREATING` -> `ACTIVE` transition for.
pub(crate) fn past_channel_activation() -> Duration {
    CHANNEL_ACTIVATION_DELAY + Duration::from_millis(400)
}

/// Look at the channel `arn` until it reports `ACTIVE` — the state its create or its last
/// accepted change owes it — through the crate's one poll loop
/// ([`crate::stream_tags::poll_until`]).
pub(crate) async fn poll_until_channel_active(emulator: &Emulator, case: &str, arn: &str) {
    crate::stream_tags::poll_until(case, &format!("the channel {arn} reports {CHANNEL_ACTIVE}"), || async {
        let (status, response) = emulator
            .call("DescribeChannel", &json!({ "ChannelARN": arn }))
            .await;
        status == 200
            && response["ChannelDescription"]["ChannelStatus"].as_str() == Some(CHANNEL_ACTIVE)
    })
    .await;
}

/// The ARN of an IAM role inside the `RoleARN` pattern
/// (`^arn:aws[-a-z0-9]*:iam::\d{12}:role/[a-zA-Z_0-9+=,.@\-_/]+$`).
pub(crate) fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", engine::EMULATOR_ACCOUNT_ID)
}

/// The ARN of a general purpose bucket inside the `BucketARN` pattern
/// (`^arn:aws[-a-z0-9]*:s3:::[a-z0-9._-]{3,63}$`).
pub(crate) fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The ARN of a table bucket inside the `TableBucketARN` pattern
/// (`^arn:aws[-a-z0-9]*:s3tables:[-a-z0-9]+:\d{12}:bucket/[a-z0-9_-]{3,63}$`).
pub(crate) fn table_bucket_arn(bucket: &str) -> String {
    format!(
        "arn:aws:s3tables:{REGION}:{}:bucket/{bucket}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN of a Glue Schema Registry schema inside the `GSRSchemaARN` pattern
/// (`^arn:aws[-a-z0-9]*:glue:[-a-z0-9]+:\d{12}:schema/[-a-zA-Z0-9_$#.]+/[-a-zA-Z0-9_$#.]+$`).
pub(crate) fn schema_arn(registry: &str, schema: &str) -> String {
    format!(
        "arn:aws:glue:{REGION}:{}:schema/{registry}/{schema}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN of a KMS key inside the key-ARN form `KeyId` documents
/// (`StartStreamEncryptionInput$KeyId`, "Key ARN example:
/// arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012").
fn key_arn(key_id: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:key/{key_id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN of a KMS alias inside the alias-ARN form `KeyId` documents (same member, "Alias ARN
/// example: arn:aws:kms:us-east-1:123456789012:alias/MyAliasName").
fn alias_arn(alias: &str) -> String {
    format!(
        "arn:aws:kms:{REGION}:{}:alias/{alias}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `S3StorageConfiguration` of a delivery bucket, carrying only the modelled required
/// members unless a case names more.
pub(crate) fn storage_configuration(bucket: &str) -> Value {
    json!({
        "BucketARN": bucket_arn(bucket),
        "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
        "CompressionType": "GZIP",
    })
}

/// A `DeadLetterQueueS3Configuration` naming its own bucket and error prefix.
pub(crate) fn dead_letter_queue(bucket: &str, prefix: &str) -> Value {
    json!({
        "BucketARN": bucket_arn(bucket),
        "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
        "ErrorOutputPrefix": prefix,
    })
}

/// The dead-letter queue a general purpose destination gets when the create names none: the
/// destination bucket under the default error prefix.
fn default_dead_letter_queue(bucket: &str) -> Value {
    json!({
        "BucketARN": bucket_arn(bucket),
        "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
        "ErrorOutputPrefix": DEFAULT_ERROR_PREFIX,
    })
}

/// One source stream configuration, naming the stream by ARN and the format its records are
/// read in.
pub(crate) fn stream_configuration(
    arn: &str,
    record_format_type: &str,
    schema: Option<String>,
) -> Value {
    let mut record_configuration = json!({ "RecordFormatType": record_format_type });
    if let Some(schema) = schema {
        record_configuration["GSRSchemaARN"] = json!(schema);
    }
    json!({ "StreamARN": arn, "RecordConfiguration": record_configuration })
}

/// An `ACTIVE` on-demand stream named `name` in `region`, which is the only capacity mode a
/// channel supports — the stream a channel case reads from. Answers its ARN.
pub(crate) async fn on_demand_stream_in(emulator: &Emulator, region: &str, name: &str) -> String {
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
    crate::stream_tags::poll_until_active_in(emulator, name, region, name).await;
    stream_arn(region, name)
}

/// [`on_demand_stream_in`] in the default test region.
pub(crate) async fn on_demand_stream(emulator: &Emulator, name: &str) -> String {
    on_demand_stream_in(emulator, REGION, name).await
}

/// The `ChannelDescription` a `CreateChannel` or `DescribeChannel` response carries.
pub(crate) fn description(case: &str, response: &Value) -> Value {
    response
        .get("ChannelDescription")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case}: the response must carry a ChannelDescription, got {response}")
        })
}

/// The channel's description with the status taken out: the status is the one member the clock
/// moves, so this is what two reads of one channel must agree on.
pub(crate) fn description_without_status(case: &str, response: &Value) -> Value {
    let mut description = description(case, response);
    if let Some(members) = description.as_object_mut() {
        members.remove("ChannelStatus");
    }
    description
}

/// A string member of a description, which the model makes required.
pub(crate) fn member(case: &str, description: &Value, name: &str) -> String {
    description
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: the description must carry {name}, got {description}")
        })
        .to_string()
}

/// One row of the create table: a channel built from `body`, whose stored record must report
/// `destination_member` as `destination`, the logging configuration `logging`, and
/// `encryption` (absent when `None`).
struct Row {
    case: &'static str,
    body: Value,
    destination_member: &'static str,
    destination: Value,
    logging: Value,
    encryption: Option<Value>,
}

/// What a create accepted is what every later read reports, with every documented default
/// resolved once by the create.
#[tokio::test]
async fn a_created_channel_is_what_a_describe_reports() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "channel-source").await;

    // The rows differ along the destination kind, the record format inside the catalog that
    // destination admits, and which optional members the create names.
    let rows: Vec<Row> = vec![
        Row {
            case: "s3-json-defaults",
            body: json!({
                "ChannelName": "s3-json-defaults",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("channel-bucket-one"),
                },
            }),
            destination_member: "S3DestinationConfiguration",
            destination: json!({
                "DataFreshnessInSeconds": DEFAULT_FRESHNESS_SECONDS,
                "DeadLetterQueueS3Configuration": default_dead_letter_queue("channel-bucket-one"),
                "StorageConfiguration": {
                    "BucketARN": bucket_arn("channel-bucket-one"),
                    "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
                    "CompressionType": "GZIP",
                    "StorageClass": DEFAULT_STORAGE_CLASS,
                },
            }),
            // A create that names no logging configuration leaves the channel logging
            // nothing, so it names no log group either.
            logging: json!({ "CloudWatchLogs": { "Enabled": false } }),
            encryption: None,
        },
        Row {
            case: "s3-string-explicit",
            body: json!({
                "ChannelName": "s3-string-explicit",
                "ServiceExecutionRoleARN": role_arn("channel/delivery.role-2"),
                "StreamConfigurationList": [stream_configuration(&source, "STRING", None)],
                "S3DestinationConfiguration": {
                    "DataFreshnessInSeconds": 60,
                    "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "failed/"),
                    "StorageConfiguration": {
                        "BucketARN": bucket_arn("channel-bucket-two"),
                        "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
                        "CompressionType": "ZSTD",
                        "StorageClass": "GLACIER_IR",
                        "OutputKeyTemplate": "records/{timestamp}",
                    },
                },
                // Logging that names no group or stream takes the documented defaults.
                "LoggingConfiguration": { "CloudWatchLogs": { "Enabled": true } },
            }),
            destination_member: "S3DestinationConfiguration",
            destination: json!({
                "DataFreshnessInSeconds": 60,
                "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "failed/"),
                "StorageConfiguration": {
                    "BucketARN": bucket_arn("channel-bucket-two"),
                    "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
                    "CompressionType": "ZSTD",
                    "StorageClass": "GLACIER_IR",
                    "OutputKeyTemplate": "records/{timestamp}",
                },
            }),
            logging: Value::Null,
            encryption: None,
        },
        Row {
            case: "s3-byte-array-encrypted",
            body: json!({
                "ChannelName": "s3-byte-array-encrypted",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&source, "BYTE_ARRAY", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("channel-bucket-three"),
                },
                "LoggingConfiguration": {
                    "CloudWatchLogs": {
                        "Enabled": true,
                        "LogGroupName": "/channels/byte-array",
                        "LogStreamName": "delivery",
                    },
                },
                "EncryptionConfiguration": {
                    "EncryptionType": "KMS",
                    "KeyId": key_arn("12345678-1234-1234-1234-123456789012"),
                },
                "Tags": { "team": "data", "env": "" },
            }),
            destination_member: "S3DestinationConfiguration",
            destination: json!({
                "DataFreshnessInSeconds": DEFAULT_FRESHNESS_SECONDS,
                "DeadLetterQueueS3Configuration": default_dead_letter_queue("channel-bucket-three"),
                "StorageConfiguration": {
                    "BucketARN": bucket_arn("channel-bucket-three"),
                    "ExpectedBucketOwner": engine::EMULATOR_ACCOUNT_ID,
                    "CompressionType": "GZIP",
                    "StorageClass": DEFAULT_STORAGE_CLASS,
                },
            }),
            logging: json!({
                "CloudWatchLogs": {
                    "Enabled": true,
                    "LogGroupName": "/channels/byte-array",
                    "LogStreamName": "delivery",
                },
            }),
            encryption: Some(json!({
                "EncryptionType": "KMS",
                "KeyId": key_arn("12345678-1234-1234-1234-123456789012"),
            })),
        },
        Row {
            case: "tables-gsr-json",
            body: json!({
                "ChannelName": "tables-gsr-json",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(
                    &source,
                    "GSR_JSON",
                    Some(schema_arn("records", "iceberg")),
                )],
                "S3TablesDestinationConfiguration": {
                    "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "tables/"),
                    "S3TablesConfigurationList": [{
                        "TableBucketARN": table_bucket_arn("channel-tables"),
                        "Namespace": "events",
                        "TableName": "records",
                        "CompressionType": "SNAPPY",
                        "PartitionSpec": {
                            "PartitionFields": [{
                                "Transform": "TIME_HOUR",
                                "SourceName": "written_at",
                            }],
                        },
                    }],
                },
            }),
            destination_member: "S3TablesDestinationConfiguration",
            destination: json!({
                "DataFreshnessInSeconds": DEFAULT_FRESHNESS_SECONDS,
                "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "tables/"),
                "S3TablesConfigurationList": [{
                    "TableBucketARN": table_bucket_arn("channel-tables"),
                    "Namespace": "events",
                    "TableName": "records",
                    "CompressionType": "SNAPPY",
                    "PartitionSpec": {
                        "PartitionFields": [{
                            "Transform": "TIME_HOUR",
                            "SourceName": "written_at",
                        }],
                    },
                }],
            }),
            logging: json!({ "CloudWatchLogs": { "Enabled": false } }),
            encryption: None,
        },
        Row {
            case: "tables-json-alias-key",
            body: json!({
                "ChannelName": "tables-json-alias-key",
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(
                    &source,
                    "JSON",
                    Some(schema_arn("records", "json")),
                )],
                "S3TablesDestinationConfiguration": {
                    "DataFreshnessInSeconds": 900,
                    "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "json/"),
                    "S3TablesConfigurationList": [{
                        "TableBucketARN": table_bucket_arn("channel-tables"),
                        "Namespace": "events",
                        "TableName": "json_records",
                        "CompressionType": "NONE",
                    }],
                },
                "EncryptionConfiguration": {
                    "EncryptionType": "KMS",
                    "KeyId": "alias/channel-key",
                },
            }),
            destination_member: "S3TablesDestinationConfiguration",
            destination: json!({
                "DataFreshnessInSeconds": 900,
                "DeadLetterQueueS3Configuration": dead_letter_queue("channel-dlq", "json/"),
                "S3TablesConfigurationList": [{
                    "TableBucketARN": table_bucket_arn("channel-tables"),
                    "Namespace": "events",
                    "TableName": "json_records",
                    "CompressionType": "NONE",
                }],
            }),
            logging: json!({ "CloudWatchLogs": { "Enabled": false } }),
            encryption: Some(json!({
                "EncryptionType": "KMS",
                "KeyId": "alias/channel-key",
            })),
        },
    ];

    for row in &rows {
        let case = row.case;
        let created = emulator.ok(case, "CreateChannel", &row.body).await;
        let description = description(case, &created);

        // "Immediately after creation, the state is CREATING"
        // (`CreateChannelOutput$ChannelDescription`).
        assert_eq!(
            member(case, &description, "ChannelStatus"),
            CREATING,
            "case {case}: expected a freshly created channel to report \
             {CREATING}, got {description}"
        );
        assert_eq!(
            member(case, &description, "ChannelName"),
            case,
            "case {case}: expected the channel to report the name it was created under, \
             got {description}"
        );
        assert_eq!(
            member(case, &description, "ChannelARN"),
            channel_arn(REGION, case),
            "case {case}: expected the ARN of the channel created in {REGION}, got \
             {description}"
        );
        assert_eq!(
            member(case, &description, "ServiceExecutionRoleARN"),
            row.body["ServiceExecutionRoleARN"].as_str().unwrap_or_default(),
            "case {case}: expected the role the create named, got {description}"
        );

        // "The unique identifier of the channel" (`ChannelDescription$ChannelId`), inside
        // its modelled `@length` of 1..64.
        let id = member(case, &description, "ChannelId");
        assert!(
            !id.is_empty() && id.chars().count() <= 64,
            "case {case}: expected a ChannelId inside its modelled length, got {id:?}"
        );
        assert!(
            description
                .get("ChannelCreationTimestamp")
                .and_then(Value::as_f64)
                .is_some_and(|at| at > 0.0),
            "case {case}: expected the creation instant the create recorded, got \
             {description}"
        );

        // The source stream is reported by the ARN it was named by, carrying the STORED
        // stream's own creation instant and the record configuration the create carried.
        let streams = description
            .get("StreamConfigurationList")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case}: the description must carry StreamConfigurationList, got {description}")
            });
        assert_eq!(
            streams.len(),
            1,
            "case {case}: expected the one source stream the create named, got {streams:?}"
        );
        let stream = &streams[0];
        assert_eq!(
            stream.get("StreamARN").and_then(Value::as_str),
            Some(source.as_str()),
            "case {case}: expected the source stream's ARN, got {stream}"
        );
        assert!(
            stream
                .get("StreamCreationTimestamp")
                .and_then(Value::as_f64)
                .is_some_and(|at| at > 0.0),
            "case {case}: expected the source stream's own creation instant, got {stream}"
        );
        assert_eq!(
            stream.get("RecordConfiguration"),
            row.body["StreamConfigurationList"][0]
                .get("RecordConfiguration"),
            "case {case}: expected the record configuration the create carried, got {stream}"
        );

        // The destination the create named, with every documented default resolved, and
        // nothing under the destination member it did not name.
        let other_member = if row.destination_member == "S3DestinationConfiguration" {
            "S3TablesDestinationConfiguration"
        } else {
            "S3DestinationConfiguration"
        };
        assert_eq!(
            description.get(row.destination_member),
            Some(&row.destination),
            "case {case}: expected {} to report the destination with its defaults \
             resolved, got {description}",
            row.destination_member
        );
        assert!(
            description.get(other_member).is_none_or(Value::is_null),
            "case {case}: expected no {other_member} on a channel created without one, got \
             {description}"
        );

        // The logging configuration, with the documented default names resolved for a
        // channel that logs: "/aws/kinesis/{channelName}/{channelId}" and
        // "DestinationDelivery".
        let expected_logging = if row.logging.is_null() {
            json!({
                "CloudWatchLogs": {
                    "Enabled": true,
                    // "Defaults to /aws/kinesis/{channelName}/{channelId}"
                    // (`CloudWatchLogs$LogGroupName`).
                    "LogGroupName": format!("/aws/kinesis/{case}/{id}"),
                    "LogStreamName": DEFAULT_LOG_STREAM_NAME,
                },
            })
        } else {
            row.logging.clone()
        };
        assert_eq!(
            description.get("LoggingConfiguration"),
            Some(&expected_logging),
            "case {case}: expected the logging configuration {expected_logging}, got \
             {description}"
        );
        assert_eq!(
            description.get("EncryptionConfiguration"),
            row.encryption.as_ref(),
            "case {case}: expected the encryption configuration the create named, got \
             {description}"
        );

        // The read projects the SAME stored record: every member but the status the clock
        // moves agrees with what the create answered.
        let described = emulator
            .ok(
                case,
                "DescribeChannel",
                &json!({ "ChannelARN": channel_arn(REGION, case) }),
            )
            .await;
        assert_eq!(
            description_without_status(case, &described),
            description_without_status(case, &created),
            "case {case}: expected a read to project the stored record the create \
             answered with, got {described}"
        );
    }
}

/// Every form a KMS key identifier may take is accepted and reported back, and the id the
/// create derived is the one every read reports.
#[tokio::test]
async fn every_documented_kms_key_form_is_accepted() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "kms-form-source").await;

    // The four forms `KeyId` documents: "a globally unique identifier, a fully specified ARN
    // to either an alias or a key, or an alias name prefixed by 'alias/'"
    // (`StartStreamEncryptionInput$KeyId`). None of them is the one the issue cited.
    let keys: Vec<(&str, String)> = vec![
        ("key-arn", key_arn("abcdef01-2345-6789-abcd-ef0123456789")),
        ("alias-arn", alias_arn("channel-alias")),
        ("key-id", "abcdef01-2345-6789-abcd-ef0123456789".to_string()),
        ("alias-name", "alias/channel_alias.2-x".to_string()),
    ];

    for (case, key_id) in &keys {
        let created = emulator
            .ok(
                case,
                "CreateChannel",
                &json!({
                    "ChannelName": case,
                    "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                    "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
                    "S3DestinationConfiguration": {
                        "StorageConfiguration": storage_configuration("channel-kms-bucket"),
                    },
                    "EncryptionConfiguration": { "EncryptionType": "KMS", "KeyId": key_id },
                }),
            )
            .await;
        let created = description(case, &created);
        assert_eq!(
            created.get("EncryptionConfiguration"),
            Some(&json!({ "EncryptionType": "KMS", "KeyId": key_id })),
            "case {case}: expected the KMS key the create named, got {created}"
        );

        let described = emulator
            .ok(
                case,
                "DescribeChannel",
                &json!({ "ChannelARN": channel_arn(REGION, case) }),
            )
            .await;
        let described = description(case, &described);
        assert_eq!(
            described.get("ChannelId"),
            created.get("ChannelId"),
            "case {case}: expected the id the create derived to be stable across reads, \
             got {described}"
        );
    }
}

/// `CREATING` is not terminal: the channel reaches `ACTIVE` once the transition the create
/// armed falls due, and nothing else about the stored record changes.
#[tokio::test]
async fn a_created_channel_becomes_active_when_the_transition_falls_due() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "activation-source").await;
    let case = "activation";

    let created = emulator
        .ok(
            case,
            "CreateChannel",
            &json!({
                "ChannelName": case,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("activation-bucket"),
                },
            }),
        )
        .await;
    let read = json!({ "ChannelARN": channel_arn(REGION, case) });

    // A caller that reads the channel straight after creating it still observes the
    // transient state.
    let described = emulator.ok(case, "DescribeChannel", &read).await;
    assert_eq!(
        member(case, &description(case, &described), "ChannelStatus"),
        CREATING,
        "case {case}: expected the channel to still be {CREATING}, got {described}"
    );

    tokio::time::sleep(past_channel_activation()).await;
    let described = emulator.ok(case, "DescribeChannel", &read).await;
    assert_eq!(
        member(case, &description(case, &described), "ChannelStatus"),
        ACTIVE,
        "case {case}: expected the channel to be {ACTIVE} once the transition fell \
         due, got {described}"
    );
    assert_eq!(
        description_without_status(case, &described),
        description_without_status(case, &created),
        "case {case}: expected the status to be the only member the clock moved, got \
         {described}"
    );
}

/// A refused create answers the modelled client error, and stores NO channel: a read of the
/// name it would have created finds nothing.
#[tokio::test]
async fn a_refused_create_stores_nothing() {
    let emulator = Emulator::new();
    let provisioned = "refusal-provisioned";
    emulator
        .ok(
            provisioned,
            "CreateStream",
            &json!({
                "StreamName": provisioned,
                "ShardCount": 1,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
            }),
        )
        .await;
    // Created before the wait inside `on_demand_stream`, so both streams are `ACTIVE` by
    // the time the cases run; the one below is created after it and stays `CREATING`.
    let source = on_demand_stream(&emulator, "refusal-source").await;
    let creating = "refusal-creating";
    emulator
        .ok(
            creating,
            "CreateStream",
            &json!({
                "StreamName": creating,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    let absent = stream_arn(REGION, "refusal-no-such-stream");

    // A create body with the destination, the source streams and the encryption a case
    // varies; everything else is the row that succeeds.
    let create = |name: &str, members: Value| -> Value {
        let mut body = json!({
            "ChannelName": name,
            "ServiceExecutionRoleARN": role_arn("channel-delivery"),
            "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
            "S3DestinationConfiguration": {
                "StorageConfiguration": storage_configuration("refusal-bucket"),
            },
        });
        if let (Some(body), Some(members)) = (body.as_object_mut(), members.as_object()) {
            for (member, value) in members {
                if value.is_null() {
                    body.remove(member);
                } else {
                    body.insert(member.clone(), value.clone());
                }
            }
        }
        body
    };

    let tables_destination = json!({
        "DeadLetterQueueS3Configuration": dead_letter_queue("refusal-dlq", "errors/"),
        "S3TablesConfigurationList": [{
            "TableBucketARN": table_bucket_arn("refusal-tables"),
            "Namespace": "events",
            "TableName": "records",
            "CompressionType": "ZSTD",
        }],
    });
    let malformed_key = "channel-key";
    let short_uuid = "12345678-1234-1234-1234-12345678901";

    // (case, body, the modelled error code, what the message must say — spelled out here,
    // with only the request- and state-derived parts interpolated, so a drift away from the
    // documented wording fails rather than comparing the handler against itself)
    let cases: Vec<(&str, Value, &str, String)> = vec![
        (
            "no-destination",
            create("no-destination", json!({ "S3DestinationConfiguration": null })),
            "InvalidArgumentException",
            "You must specify either S3DestinationConfiguration or \
             S3TablesDestinationConfiguration, but not both."
                .to_string(),
        ),
        (
            "both-destinations",
            create(
                "both-destinations",
                json!({ "S3TablesDestinationConfiguration": tables_destination.clone() }),
            ),
            "InvalidArgumentException",
            "You must specify either S3DestinationConfiguration or \
             S3TablesDestinationConfiguration, but not both."
                .to_string(),
        ),
        (
            "two-source-streams",
            create(
                "two-source-streams",
                json!({
                    "StreamConfigurationList": [
                        stream_configuration(&source, "JSON", None),
                        stream_configuration(&stream_arn(REGION, creating), "JSON", None),
                    ],
                }),
            ),
            "InvalidArgumentException",
            "currently 1 stream is supported per channel".to_string(),
        ),
        (
            "unheld-source-stream",
            create(
                "unheld-source-stream",
                json!({
                    "StreamConfigurationList": [stream_configuration(&absent, "JSON", None)],
                }),
            ),
            "ResourceNotFoundException",
            format!(
                "Stream refusal-no-such-stream under account {} not found.",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "source-stream-still-creating",
            create(
                "source-stream-still-creating",
                json!({
                    "StreamConfigurationList": [stream_configuration(
                        &stream_arn(REGION, creating),
                        "JSON",
                        None,
                    )],
                }),
            ),
            "ResourceInUseException",
            // The sentence this service states for a stream that is not `ACTIVE`, written
            // out here rather than borrowed from the crate, so the wording is what this
            // case pins.
            format!(
                "Stream {creating} under account {} not ACTIVE, instead in state CREATING",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "provisioned-source-stream",
            create(
                "provisioned-source-stream",
                json!({
                    "StreamConfigurationList": [stream_configuration(
                        &stream_arn(REGION, provisioned),
                        "JSON",
                        None,
                    )],
                }),
            ),
            "ValidationException",
            format!(
                "stream {} is in {STREAM_MODE_PROVISIONED} mode; a channel is only \
                 supported for data streams with the {STREAM_MODE_ON_DEMAND} capacity mode",
                stream_arn(REGION, provisioned)
            ),
        ),
        (
            "malformed-kms-key",
            create(
                "malformed-kms-key",
                json!({
                    "EncryptionConfiguration": {
                        "EncryptionType": "KMS",
                        "KeyId": malformed_key,
                    },
                }),
            ),
            "ValidationException",
            format!("Invalid KMS key identifier {malformed_key}"),
        ),
        (
            "truncated-kms-key-id",
            create(
                "truncated-kms-key-id",
                json!({
                    "EncryptionConfiguration": {
                        "EncryptionType": "KMS",
                        "KeyId": short_uuid,
                    },
                }),
            ),
            "ValidationException",
            format!("Invalid KMS key identifier {short_uuid}"),
        ),
        (
            "service-kms-key-alias",
            create(
                "service-kms-key-alias",
                json!({
                    "EncryptionConfiguration": {
                        "EncryptionType": "KMS",
                        "KeyId": SERVICE_KEY_ALIAS,
                    },
                }),
            ),
            "ValidationException",
            "You cannot use the Amazon Kinesis Data Streams service key (aws/kinesis)"
                .to_string(),
        ),
        (
            "service-kms-key-arn",
            create(
                "service-kms-key-arn",
                json!({
                    "EncryptionConfiguration": {
                        "EncryptionType": "KMS",
                        "KeyId": alias_arn("aws/kinesis"),
                    },
                }),
            ),
            "ValidationException",
            "You cannot use the Amazon Kinesis Data Streams service key (aws/kinesis)"
                .to_string(),
        ),
        (
            "string-records-to-tables",
            create(
                "string-records-to-tables",
                json!({
                    "S3DestinationConfiguration": null,
                    "S3TablesDestinationConfiguration": tables_destination.clone(),
                    "StreamConfigurationList": [stream_configuration(
                        &source,
                        "STRING",
                        Some(schema_arn("records", "strings")),
                    )],
                }),
            ),
            "InvalidArgumentException",
            "RecordFormatType STRING is not supported for a S3_TABLES destination".to_string(),
        ),
        (
            "gsr-records-to-bucket",
            create(
                "gsr-records-to-bucket",
                json!({
                    "StreamConfigurationList": [stream_configuration(
                        &source,
                        "GSR_JSON",
                        Some(schema_arn("records", "gsr")),
                    )],
                }),
            ),
            "InvalidArgumentException",
            "RecordFormatType GSR_JSON is not supported for a S3 destination".to_string(),
        ),
        (
            "tables-without-schema",
            create(
                "tables-without-schema",
                json!({
                    "S3DestinationConfiguration": null,
                    "S3TablesDestinationConfiguration": tables_destination.clone(),
                    "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
                }),
            ),
            "InvalidArgumentException",
            "RecordConfiguration.GSRSchemaARN is required when the channel destination is a \
             streaming table."
                .to_string(),
        ),
    ];

    for (case, body, code, says) in &cases {
        let (status, response) = emulator.call("CreateChannel", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            *code,
            "case {case}: expected {code}, got {response}"
        );
        let message = error_message(&response);
        assert!(
            message.contains(says.as_str()),
            "case {case}: expected the message to name {says:?}, got {message:?}"
        );

        // Nothing was stored, so the name the refused create named holds no channel.
        let (status, response) = emulator
            .call(
                "DescribeChannel",
                &json!({ "ChannelARN": channel_arn(REGION, case) }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the refused create to have stored nothing, got {status} \
             with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case}: expected the refused create to have stored nothing, got {response}"
        );
    }
}

/// A channel name is unique within one account and region: a second create under a name the
/// region holds is refused and the stored channel stands, and the same name in another region
/// is another channel.
#[tokio::test]
async fn a_channel_name_is_unique_within_a_region() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "unique-source").await;
    let case = "unique-channel";
    let create = |role: &str| {
        json!({
            "ChannelName": case,
            "ServiceExecutionRoleARN": role_arn(role),
            "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
            "S3DestinationConfiguration": {
                "StorageConfiguration": storage_configuration("unique-bucket"),
            },
        })
    };

    emulator
        .ok(case, "CreateChannel", &create("channel-delivery"))
        .await;
    let (status, response) = emulator
        .call("CreateChannel", &create("channel-second-role"))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {case}: expected ResourceInUseException, got {response}"
    );
    let message = error_message(&response);
    let says = format!(
        "Channel {case} under account {} already exists.",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        message.contains(&says),
        "case {case}: expected the message to name {says:?}, got {message:?}"
    );

    // The refused create left the stored channel exactly as it was.
    let described = emulator
        .ok(
            case,
            "DescribeChannel",
            &json!({ "ChannelARN": channel_arn(REGION, case) }),
        )
        .await;
    assert_eq!(
        member(case, &description(case, &described), "ServiceExecutionRoleARN"),
        role_arn("channel-delivery"),
        "case {case}: expected the refused create to leave the stored channel as it was, \
         got {described}"
    );

    // Another region holds no channel under that name, and its own create is another
    // channel with its own ARN.
    let elsewhere = "us-west-2";
    let (status, response) = emulator
        .call_in(
            elsewhere,
            "DescribeChannel",
            &json!({ "ChannelARN": channel_arn(REGION, case) }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected {elsewhere} to hold no channel of {REGION}, got {status} with \
         body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case {case}: expected ResourceNotFoundException, got {response}"
    );
    let says = format!(
        "Channel {case} under account {} not found.",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        error_message(&response).contains(&says),
        "case {case}: expected the message to name {says:?}, got {response}"
    );

    let source_elsewhere = on_demand_stream_in(&emulator, elsewhere, "unique-source").await;
    let created = emulator
        .ok_in(
            case,
            elsewhere,
            "CreateChannel",
            &json!({
                "ChannelName": case,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&source_elsewhere, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("unique-bucket"),
                },
            }),
        )
        .await;
    assert_eq!(
        member(case, &description(case, &created), "ChannelARN"),
        channel_arn(elsewhere, case),
        "case {case}: expected the channel created in {elsewhere} to carry that region's \
         ARN, got {created}"
    );
}

/// Of any number of overlapping creates of ONE name, exactly one is accepted: the create
/// takes the store's lock for the whole read-and-insert, so a second one finds the name taken.
#[tokio::test]
async fn overlapping_creates_of_one_name_admit_exactly_one() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "race-source").await;
    let case = "race-channel";
    let body = json!({
        "ChannelName": case,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
        "S3DestinationConfiguration": {
            "StorageConfiguration": storage_configuration("race-bucket"),
        },
    });

    let answers = tokio::join!(
        emulator.call("CreateChannel", &body),
        emulator.call("CreateChannel", &body),
        emulator.call("CreateChannel", &body),
    );
    let answers = [answers.0, answers.1, answers.2];
    let accepted = answers.iter().filter(|(status, _)| *status == 200).count();
    assert_eq!(
        accepted, 1,
        "case {case}: expected exactly one winner among {} overlapping creates, got \
         {answers:?}",
        answers.len()
    );
    for (status, response) in answers.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 400,
            "case {case}: expected the losers to answer 400, got {status} with body {response}"
        );
        assert_eq!(
            error_code(response),
            "ResourceInUseException",
            "case {case}: expected the loser to be told the name is taken, got {response}"
        );
    }
}

/// The store is bounded: a create past the channels one region may hold is refused with the
/// modelled `LimitExceededException` and stores nothing, while the channels already held are
/// still readable.
#[tokio::test]
async fn a_create_past_the_region_ceiling_is_refused() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "ceiling-source").await;
    let create = |name: &str| {
        json!({
            "ChannelName": name,
            "ServiceExecutionRoleARN": role_arn("channel-delivery"),
            "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
            "S3DestinationConfiguration": {
                "StorageConfiguration": storage_configuration("ceiling-bucket"),
            },
        })
    };

    for held in 0..MAX_CHANNELS_PER_REGION {
        let name = format!("ceiling-{held}");
        emulator.ok(&name, "CreateChannel", &create(&name)).await;
    }

    let case = "ceiling-past";
    let (status, response) = emulator.call("CreateChannel", &create(case)).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a client error once the region is full, got {status} with \
         body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {case}: expected LimitExceededException, got {response}"
    );
    let message = error_message(&response);
    let says = format!("this region holds {MAX_CHANNELS_PER_REGION} channels");
    assert!(
        message.contains(&says),
        "case {case}: expected the message to name {says:?}, got {message:?}"
    );

    // The refused create stored nothing, and the channels the region already held are
    // untouched.
    let (status, response) = emulator
        .call(
            "DescribeChannel",
            &json!({ "ChannelARN": channel_arn(REGION, case) }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected the refused create to have stored nothing, got {status} with \
         body {response}"
    );
    let first = "ceiling-0";
    emulator
        .ok(
            first,
            "DescribeChannel",
            &json!({ "ChannelARN": channel_arn(REGION, first) }),
        )
        .await;
}
