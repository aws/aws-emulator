// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesis-kinesis-20131202 service.
//!
//! This module owns the stream records the implemented operations read and write: the
//! shard map `CreateStream` lays down, the ARN a stream is addressed by, the tags assigned
//! to it, the record log a producer appends to and a consumer drains, the enhanced fan-out
//! consumers registered with it ([`ConsumerRecord`]), and the single place where the
//! `CREATING` -> `ACTIVE`, `UPDATING` -> `ACTIVE` and `DELETING` -> gone transitions the
//! clock owes a stream are decided ([`Stream::status`], [`Stream::is_held`]).
//!
//! It owns the channel records the same way ([`Channel`]): the destination, logging and
//! encryption configuration `CreateChannel` resolves every documented default into, the ARN
//! and id a channel is addressed by, and the `CREATING` -> `ACTIVE` and `UPDATING` ->
//! `ACTIVE` transitions the clock owes it ([`Channel::status`]).
//!
//! It owns this account's minimum throughput billing commitment the same way
//! ([`Commitment`]): the instant an enable started it, the earliest instant it may end at,
//! and the end a disable scheduled for it, with the `ENABLED` ->
//! `ENABLED_UNTIL_EARLIEST_ALLOWED_END` -> `DISABLED` transitions decided in one place
//! ([`Commitment::status`]).

//! Three concerns of that state live in their own files beside this one, because each is a
//! rule of its own that several operations consult: the per-shard WRITE quota a producer is
//! metered by ([`write_quota`]), what KMS says about the key a stream is encrypted with
//! ([`kms_key`]), and whether a channel's service execution role may use that key
//! ([`channel_key_access`]).

mod channel_key_access;
mod kms_key;
mod write_quota;

pub use channel_key_access::*;
pub use kms_key::*;
pub use write_quota::*;

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ChannelDescription, ChannelEncryptionConfiguration, ChannelLoggingConfiguration,
    ChannelStreamDescription, ChannelStreamIdentifier, ChannelSummary, ChildShard, CloudWatchLogs,
    Consumer, ConsumerDescription, EnhancedMetrics, HashKeyRange,
    MinimumThroughputBillingCommitmentOutput, Record, S3DestinationDescription,
    S3TablesDestinationDescription, SequenceNumberRange, Shard, StreamDescription,
    StreamDescriptionSummary, StreamModeDetails, StreamSummary, Tag, WarmThroughputObject,
    METRICS_NAME_VALUES,
};

/// The region a stream is created in when the request carries no SigV4 credential scope
/// to read one from (the emulator's default region, as `engine::emulator_arn` documents).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, which is also the service segment the `StreamARN`
/// pattern (`^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`) requires.
pub const SERVICE_ARN_NAMESPACE: &str = "kinesis";

/// A stream that is still being created, spelled as `StreamStatus` models it
/// (`smithy_query(query="shape",shape="StreamStatus")`).
pub const STREAM_CREATING: &str = "CREATING";

/// A stream that is ready for read and write operations.
pub const STREAM_ACTIVE: &str = "ACTIVE";

/// A stream a `DeleteStream` has been accepted for, which is gone once the deletion the
/// call armed completes: "After a DeleteStream request, the specified stream is in the
/// DELETING state until Kinesis Data Streams completes the deletion" (`DeleteStream`).
pub const STREAM_DELETING: &str = "DELETING";

/// A stream whose shard map is being re-tiled: "Upon receiving the request, Kinesis Data
/// Streams returns immediately and sets the status of the stream to UPDATING. After the
/// update is complete, Kinesis Data Streams sets the status of the stream back to ACTIVE"
/// (`UpdateShardCount`).
pub const STREAM_UPDATING: &str = "UPDATING";

/// The `StreamMode` whose streams carry the shard count the caller asked for.
pub const STREAM_MODE_PROVISIONED: &str = "PROVISIONED";

/// The `StreamMode` whose shard count the service manages itself.
pub const STREAM_MODE_ON_DEMAND: &str = "ON_DEMAND";

/// The capacity mode a stream gets when the request names no `StreamModeDetails`: the
/// `CreateStream` doc makes the shard count the caller's to choose ("For the data streams
/// with a provisioned mode, you must specify the number of shards"), which is the
/// provisioned mode.
pub const DEFAULT_STREAM_MODE: &str = STREAM_MODE_PROVISIONED;

/// How long after the `CreateStream` call the stream's `CREATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration: it only has to be non-zero,
/// so a caller that reads the stream straight after creating it still observes
/// `CREATING`. The instant it falls due is stored on the record and never slept on.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after the `DeleteStream` call the stream's removal falls due, taking it from
/// `DELETING` to gone. A CHOSEN span, not an AWS-documented duration: it only has to be
/// non-zero, so a caller that reads the stream straight after deleting it still observes
/// `DELETING`. The instant it falls due is stored on the record and never slept on.
pub const DELETION_DELAY: Duration = Duration::from_millis(500);

/// How long after a shard-count change the stream's `UPDATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration ("the scaling action could
/// take a few minutes to complete" names no value): it only has to be non-zero, so a
/// caller that reads the stream straight after scaling it still observes `UPDATING`. The
/// instant it falls due is stored on the record and never slept on.
pub const RESHARD_DELAY: Duration = Duration::from_millis(500);

/// How long after a capacity-mode change the stream's `UPDATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration: it only has to be non-zero,
/// so a caller that reads the stream straight after the change still observes `UPDATING`,
/// and a second change that overlaps the first is refused rather than served. The instant
/// it falls due is stored on the record and never slept on.
pub const MODE_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// How long after a warm-throughput change the stream's `UPDATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration ("Depending on the size of the
/// stream, the scaling action could take a few minutes to complete" names no value): it only
/// has to be non-zero, so a caller that reads the stream straight after the change still
/// observes `UPDATING`, and a second change that overlaps the first is refused rather than
/// served. The instant it falls due is stored on the record and never slept on.
pub const WARM_THROUGHPUT_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// The highest warm throughput a stream may be scaled to, in MiB/s: "By default, you cannot
/// do the following: Scale to more than 10 GiBps for an on-demand stream"
/// (`UpdateStreamWarmThroughput`). A DOCUMENTED ceiling, expressed in the MiB/s the
/// `WarmThroughputMiBps` member is stated in, so the 10 GiB/s it names is this many MiB/s.
pub const MAX_WARM_THROUGHPUT_MI_BPS: i32 = 10 * 1024;

/// An account that holds a standing minimum throughput billing commitment, spelled as
/// `MinimumThroughputBillingCommitmentOutputStatus` models it
/// (`smithy_query(query="shape",shape="MinimumThroughputBillingCommitmentOutputStatus")`).
pub const COMMITMENT_ENABLED: &str = "ENABLED";

/// An account that holds no standing minimum throughput billing commitment, which is what
/// an account that has never asked for one carries.
pub const COMMITMENT_DISABLED: &str = "DISABLED";

/// A standing commitment a disable has already been accepted for, which stands until its
/// earliest allowed end: "If you attempt to disable a commitment before the minimum
/// commitment period ends, the commitment will be scheduled for automatic disable at the
/// earliest allowed end time" (`MinimumThroughputBillingCommitmentInput`).
pub const COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END: &str =
    "ENABLED_UNTIL_EARLIEST_ALLOWED_END";

/// How long a minimum throughput billing commitment stands before it may end: "Commitments
/// have a minimum duration of 24 hours that must be honored before they can be disabled"
/// (`MinimumThroughputBillingCommitmentInput`). A DOCUMENTED duration, so it keeps the value
/// the doc states rather than a span this emulator chooses.
pub const MINIMUM_COMMITMENT_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// A consumer that is still being registered, spelled as `ConsumerStatus` models it
/// (`smithy_query(query="shape",shape="ConsumerStatus")`). "A consumer can't read data
/// while in the CREATING or DELETING states" (`Consumer$ConsumerStatus`).
pub const CONSUMER_CREATING: &str = "CREATING";

/// A consumer that is ready to read the stream it is registered with through enhanced
/// fan-out.
pub const CONSUMER_ACTIVE: &str = "ACTIVE";

/// How long after the `RegisterStreamConsumer` call the consumer's `CREATING` -> `ACTIVE`
/// transition falls due. A CHOSEN span, not an AWS-documented duration: it only has to be
/// non-zero, so a caller that reads the consumer straight after registering it still
/// observes `CREATING`, and it is the same span this emulator gives a stream's own
/// activation ([`ACTIVATION_DELAY`]). The instant it falls due is stored on the record and
/// never slept on.
pub const CONSUMER_ACTIVATION_DELAY: Duration = ACTIVATION_DELAY;

/// The most consumers one stream carries: "With On-demand Standard and Provisioned
/// streams, you can register up to 20 consumers per stream to use Enhanced Fan-out"
/// (`RegisterStreamConsumer`) — the mode every stream of this emulator is in, since
/// On-demand Advantage (50 consumers) is a billing option no operation of this service
/// exposes.
pub const MAX_CONSUMERS_PER_STREAM: usize = 20;

/// The most consumers of one stream that may be in `CREATING` at the same time: "only 5
/// consumers can be created simultaneously. In other words, you cannot have more than 5
/// consumers in a CREATING status at the same time. Registering a 6th consumer while there
/// are 5 in a CREATING status results in a LimitExceededException"
/// (`RegisterStreamConsumer`).
pub const MAX_CONSUMERS_CREATING: usize = 5;

/// The most tags one consumer carries: "A set of up to 50 key-value pairs"
/// (`RegisterStreamConsumerInput$Tags`), the same count a stream carries
/// ([`MAX_STREAM_TAGS`]) and narrower than the `TagMap` `@length` of 1..200 the boundary
/// validator enforces.
pub const MAX_CONSUMER_TAGS: usize = MAX_STREAM_TAGS;

/// How many shard-count changes one stream may be scaled by inside [`RESHARD_WINDOW`]: by
/// default you cannot "scale more than ten times per rolling 24-hour period per stream"
/// (`UpdateShardCount`).
pub const MAX_RESHARDS_PER_WINDOW: usize = 10;

/// The rolling window [`MAX_RESHARDS_PER_WINDOW`] scaling actions are counted over: the
/// "rolling 24-hour period per stream" `UpdateShardCount` documents.
pub const RESHARD_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The retention window a new stream gets, in hours: "By default, a stream's data records
/// are accessible for 24 hours from the time they are added to the stream"
/// (`IncreaseStreamRetentionPeriod`), which is also the minimum
/// `StreamDescription$RetentionPeriodHours` publishes.
pub const DEFAULT_RETENTION_PERIOD_HOURS: i32 = 24;

/// The shortest retention window a stream may carry, in hours: "The minimum value of a
/// stream's retention period is 24 hours" (`DecreaseStreamRetentionPeriod`), which is also
/// the window a stream starts with ([`DEFAULT_RETENTION_PERIOD_HOURS`]).
pub const MIN_RETENTION_PERIOD_HOURS: i32 = DEFAULT_RETENTION_PERIOD_HOURS;

/// The longest retention window a stream may carry, in hours: "The maximum value of a
/// stream's retention period is 8760 hours (365 days)"
/// (`IncreaseStreamRetentionPeriod`).
pub const MAX_RETENTION_PERIOD_HOURS: i32 = 8_760;

/// The shards an on-demand stream starts with. The capacity an on-demand stream starts
/// with is 4 MiB/s of writes, and a shard supports "writes up to 1,000 records per
/// second, up to a maximum data write total of 1 MiB per second" (`CreateStream`), so a
/// new on-demand stream's shard map holds four shards.
pub const ON_DEMAND_INITIAL_SHARDS: u32 = 4;

/// The write throughput one shard supports, in MiB/s: "Each shard can support writes up
/// to 1,000 records per second, up to a maximum data write total of 1 MiB per second"
/// (`CreateStream`). A stream's warm throughput is this times its open shard count.
pub const SHARD_WRITE_MI_BPS: i32 = 1;

/// The server-side encryption a new stream carries: `EncryptionType` is `NONE | KMS` and
/// `StartStreamEncryption` is what moves a stream to `KMS`, so a stream nothing has
/// encrypted reports `NONE`.
pub const ENCRYPTION_TYPE_NONE: &str = "NONE";

/// The server-side encryption a stream carries once it is encrypted with a KMS key, which
/// is also the only `EncryptionType` either encryption operation accepts: "The encryption
/// type. The only valid value is KMS" (`StopStreamEncryptionInput$EncryptionType`).
pub const ENCRYPTION_TYPE_KMS: &str = "KMS";

/// The `EncryptionType` a data-plane answer reports for data the service encrypted under
/// `encryption_type`, and nothing at all when it encrypted nothing.
///
/// The member is optional wherever it reports encryption APPLIED to data — "The encryption
/// type to use on the record" (`PutRecordOutput$EncryptionType`), "The encryption type used
/// on the records" (`PutRecordsOutput$EncryptionType`) — so its presence is what tells a
/// producer that server-side encryption took effect, and a write to a stream with no
/// encryption reports no type. A stream's own description reports the CONFIGURATION instead
/// ("The encryption type used", `StreamDescriptionSummary$EncryptionType`), which every
/// stream has, so that member keeps naming `NONE`.
pub fn applied_encryption_type(encryption_type: &str) -> Option<String> {
    (encryption_type != ENCRYPTION_TYPE_NONE).then(|| encryption_type.to_string())
}

/// How long after an encryption change the stream's `UPDATING` -> `ACTIVE` transition falls
/// due. A CHOSEN span, not an AWS-documented duration ("Stopping encryption normally takes a
/// few seconds to complete, but it can take minutes" names no value): it only has to be
/// non-zero, so a caller that reads the stream straight after the change still observes
/// `UPDATING`. The instant it falls due is stored on the record and never slept on.
pub const ENCRYPTION_UPDATE_DELAY: Duration = Duration::from_millis(500);

/// How many times ONE kind of encryption change may be made to one stream inside
/// [`ENCRYPTION_CHANGE_WINDOW`]: "You can successfully disable server-side encryption 25
/// times in a rolling 24-hour period" (`StopStreamEncryption`) and "You can successfully
/// apply a new Amazon Web Services KMS key for server-side encryption 25 times in a rolling
/// 24-hour period" (`StartStreamEncryption`) — one ceiling per kind, so enabling and
/// disabling are counted separately against it.
pub const MAX_ENCRYPTION_CHANGES_PER_WINDOW: usize = 25;

/// The rolling window [`MAX_ENCRYPTION_CHANGES_PER_WINDOW`] encryption changes are counted
/// over: the "rolling 24-hour period" both encryption operations document.
pub const ENCRYPTION_CHANGE_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// The shard quota this emulator authorizes one account and region, which is the ONE
/// ceiling every operation that opens a shard is refused past and the ceiling
/// `DescribeLimits` publishes as `ShardLimit`
/// ([`ComAmazonawsKinesisKinesis20131202State::shard_quota_in`]).
///
/// The `CreateStream` doc answers `LimitExceededException` when a request would "create
/// more shards than are authorized for your account", and the number authorized is the one
/// `DescribeLimits` reports, so there is no second, per-stream ceiling: a stream may hold
/// every shard the account's quota admits. Set at the order of AWS's own default per-region
/// shard quota.
pub const MAX_SHARDS_PER_REGION: u32 = 1_000;

/// The most streams of the on-demand capacity mode one region of this emulator holds
/// across every stream, which is the quota `DescribeLimits` reports as
/// `OnDemandStreamCountLimit`.
///
/// `CreateStream` answers `LimitExceededException` for a request past "the default shard or
/// on-demand throughput limits for an Amazon Web Services account" and names no number for
/// them, so this is the on-demand stream count this emulator authorizes — its OWN ceiling,
/// at the order of AWS's own default per-account and region on-demand stream quota, and
/// refused past by every operation that gives a stream that capacity mode.
pub const MAX_ON_DEMAND_STREAMS_PER_REGION: usize = 50;

/// The most shards this emulator holds across EVERY region, whatever region each was
/// created in — the largest thing a create allocates, so this is the ceiling that bounds
/// the store's memory.
///
/// The shard quota above is partitioned by the region the CALLER's credential scope names,
/// which the caller chooses, so it bounds no total on its own: this ceiling, which no
/// request can partition, is what keeps the store from growing without end, and a stream
/// always holds at least one shard, so it bounds the entry count too. The emulator's OWN
/// ceiling, not a modelled limit — and it is folded INTO the quota `DescribeLimits`
/// publishes ([`ComAmazonawsKinesisKinesis20131202State::shard_quota_in`]), so the ceiling
/// reported and the ceiling enforced stay one number.
pub const MAX_SHARDS_TOTAL: u32 = 4_000;

/// The most tags one stream carries: "A set of up to 50 key-value pairs to use to create
/// the tags" (`CreateStreamInput$Tags`), which is narrower than the `TagMap` `@length` of
/// 1..200 the boundary validator enforces.
pub const MAX_STREAM_TAGS: usize = 50;

/// The most tags `ListTagsForStream` returns in one call, which is also the page a request
/// naming no `Limit` gets: `Limit` is modelled `range 1..50`
/// (`smithy_query(query="operation",operation="ListTagsForStream")`), and a stream carries
/// at most [`MAX_STREAM_TAGS`] tags, so a request naming no limit gets all of them.
pub const MAX_LIST_TAGS_FOR_STREAM_LIMIT: i32 = 50;

/// How many shards `DescribeStream` returns when the request names no `Limit`: "The
/// maximum number of shards to return in a single call. The default value is 100"
/// (`DescribeStreamInput$Limit`).
pub const DEFAULT_DESCRIBE_STREAM_SHARD_LIMIT: i32 = 100;

/// The most shards `DescribeStream` returns in one call: "If you specify a value greater
/// than 100, at most 100 results are returned" (`DescribeStreamInput$Limit`).
pub const MAX_DESCRIBE_STREAM_SHARD_LIMIT: i32 = 100;

/// How many shards `ListShards` returns when the request names no `MaxResults`: "The
/// maximum number of shards to return in a single call. The default value is 1000"
/// (`ListShardsInput$MaxResults`).
pub const DEFAULT_LIST_SHARDS_LIMIT: i32 = 1_000;

/// The most shards `ListShards` returns in one call: "If you specify a value greater than
/// 1000, at most 1000 results are returned" (`ListShardsInput$MaxResults`), which the
/// modelled `range 1..10000` deliberately leaves room above.
pub const MAX_LIST_SHARDS_LIMIT: i32 = 1_000;

/// How long a pagination token of this service stays usable. `ListShards` is the operation
/// that publishes the lifetime — "Tokens expire after 300 seconds. When you obtain a value
/// for NextToken in the response to a call to ListShards, you have 300 seconds to use that
/// value" (`ListShardsOutput$NextToken`) — and a cursor of this service is a cursor
/// whichever listing minted it, so every one of them has that lifetime.
pub const PAGE_TOKEN_TTL: Duration = Duration::from_secs(300);

/// See [`PAGE_TOKEN_TTL`]: how long a `ListShards` token stays usable.
pub const LIST_SHARDS_TOKEN_TTL: Duration = PAGE_TOKEN_TTL;

/// How many streams `ListStreams` returns when the request names no `Limit`: "The maximum
/// number of streams to list. The default value is 100" (`ListStreamsInput$Limit`).
pub const DEFAULT_LIST_STREAMS_LIMIT: i32 = 100;

/// The most streams `ListStreams` returns in one call: "If you specify a value greater than
/// 100, at most 100 results are returned" (`ListStreamsInput$Limit`), which the modelled
/// `range 1..10000` deliberately leaves room above.
pub const MAX_LIST_STREAMS_LIMIT: i32 = 100;

/// How many consumers `ListStreamConsumers` returns when the request names no `MaxResults`:
/// "The maximum number of consumers that you want a single call of ListStreamConsumers to
/// return. The default value is 100" (`ListStreamConsumersInput$MaxResults`).
pub const DEFAULT_LIST_STREAM_CONSUMERS_LIMIT: i32 = 100;

/// The most consumers `ListStreamConsumers` returns in one call: "If you specify a value
/// greater than 100, at most 100 results are returned"
/// (`ListStreamConsumersInput$MaxResults`), which the modelled `range 1..10000`
/// deliberately leaves room above.
pub const MAX_LIST_STREAM_CONSUMERS_LIMIT: i32 = 100;

/// How many records `GetRecords` returns when the request names no `Limit`: "The maximum
/// number of records to return. Specify a value of up to 10,000... The default value is
/// 10,000" (`GetRecordsInput$Limit`).
pub const DEFAULT_GET_RECORDS_LIMIT: i32 = 10_000;

/// The most records `GetRecords` returns in one call: "The maximum number of records that
/// can be returned per call is 10,000" (`GetRecords`).
pub const MAX_GET_RECORDS_LIMIT: i32 = 10_000;

/// The most bytes one `GetRecords` call answers with: "The maximum size of data that
/// GetRecords can return is 10 MiB" (`GetRecords`), so a page stops at this size even when
/// the limit would admit more records.
pub const MAX_GET_RECORDS_BYTES: usize = 10 * 1024 * 1024;

/// The most records one shard of this emulator holds. A record stays in the store until
/// its retention window passes, so the shard needs a ceiling its oldest records are
/// dropped at; it is the emulator's OWN ceiling, set at the most one `GetRecords` call can
/// return ([`MAX_GET_RECORDS_LIMIT`]).
pub const MAX_RECORDS_PER_SHARD: usize = 10_000;

/// The most record bytes one shard of this emulator holds, the second half of the ceiling
/// [`MAX_RECORDS_PER_SHARD`] opens: `Data` is modelled up to 10 MiB per record, so a count
/// alone does not bound the store's memory. Set above one maximum-size record, because the
/// record a write just stored is never the one the ceiling drops. With
/// [`MAX_SHARDS_TOTAL`] bounding how many shards exist at all, the two together bound what
/// every stream of every region holds. The emulator's OWN ceiling, not a modelled limit.
pub const MAX_RECORD_BYTES_PER_SHARD: usize = 16 * 1024 * 1024;

/// A channel that is still being provisioned: "CREATING - The channel is being created"
/// (`ChannelSummary$ChannelStatus`).
pub const CHANNEL_CREATING: &str = "CREATING";

/// A channel that is ready to deliver records: "ACTIVE - The channel is ready to deliver
/// records" (`ChannelSummary$ChannelStatus`).
pub const CHANNEL_ACTIVE: &str = "ACTIVE";

/// A channel whose accepted configuration change is still being applied: "UPDATING - The
/// channel configuration is being updated" (`ChannelDescription$ChannelStatus`).
pub const CHANNEL_UPDATING: &str = "UPDATING";

/// How long after the `CreateChannel` call a channel's `CREATING` -> `ACTIVE` transition
/// falls due. A CHOSEN span, not an AWS-documented duration ("Creating a channel is an
/// asynchronous operation. Upon receiving the request, Amazon Kinesis Data Streams returns
/// immediately with the channel in the CREATING state. After provisioning is complete,
/// Amazon Kinesis Data Streams sets the state to ACTIVE" names no value): it only has to be
/// non-zero, so a caller that describes the channel straight after creating it still
/// observes `CREATING`. The instant it falls due is stored on the record and never slept on.
pub const CHANNEL_ACTIVATION_DELAY: Duration = Duration::from_secs(3);

/// How long after an accepted `UpdateChannel` call the channel's `UPDATING` -> `ACTIVE`
/// transition falls due. A CHOSEN span, not an AWS-documented duration ("Updating a channel
/// is an asynchronous operation. Upon receiving the request, Amazon Kinesis Data Streams sets
/// the channel to the UPDATING state and returns immediately. After the change is applied,
/// Amazon Kinesis Data Streams sets the channel back to the ACTIVE state" names no value):
/// it only has to be non-zero, so a caller that describes the channel straight after
/// updating it still observes `UPDATING`, and a second change that overlaps the first is
/// refused rather than served. The instant it falls due is stored on the record and never
/// slept on.
pub const CHANNEL_UPDATE_DELAY: Duration = Duration::from_secs(3);

/// A channel that delivers to a general purpose bucket: "S3 - Delivery to a general purpose
/// Amazon S3 bucket" (`ChannelSummary$ChannelDestinationType`).
pub const CHANNEL_DESTINATION_S3: &str = "S3";

/// A channel that delivers to streaming tables: "S3_TABLES - Delivery to streaming tables
/// on Apache Iceberg" (`ChannelSummary$ChannelDestinationType`).
pub const CHANNEL_DESTINATION_S3_TABLES: &str = "S3_TABLES";

/// The record formats a general purpose bucket destination admits: "JSON - Supported for
/// both general purpose Amazon S3 and streaming table destinations", "STRING - Supported
/// only for general purpose Amazon S3 destinations", "BYTE_ARRAY - Supported only for
/// general purpose Amazon S3 destinations" (`RecordConfiguration$RecordFormatType`).
pub const S3_RECORD_FORMAT_TYPES: &[&str] = &["JSON", "STRING", "BYTE_ARRAY"];

/// The record formats a streaming table destination admits: "GSR_JSON - Supported only for
/// streaming table (Amazon S3 Tables) destinations" and the `JSON` both destinations admit
/// (`RecordConfiguration$RecordFormatType`).
pub const S3_TABLES_RECORD_FORMAT_TYPES: &[&str] = &["GSR_JSON", "JSON"];

/// How many source streams one channel carries: "Currently, one stream is supported per
/// channel" (`CreateChannelInput$StreamConfigurationList`), narrower than that member's
/// modelled `@length` of 1..10000, which the boundary validator enforces.
pub const MAX_STREAMS_PER_CHANNEL: usize = 1;

/// The maximum age of undelivered data a destination gets when the request names none: "The
/// maximum age, in seconds, of undelivered data before the channel delivers it to the
/// destination. The default value is 300 seconds"
/// (`S3DestinationConfiguration$DataFreshnessInSeconds`).
pub const DEFAULT_DATA_FRESHNESS_SECONDS: i32 = 300;

/// The storage class delivered objects get when the request names none: "STANDARD - The
/// default storage class, for frequently accessed data"
/// (`S3StorageConfiguration$StorageClass`).
pub const DEFAULT_S3_STORAGE_CLASS: &str = "STANDARD";

/// The log stream a channel's CloudWatch logging uses when the request names none:
/// "Defaults to DestinationDelivery" (`CloudWatchLogs$LogStreamName`).
pub const DEFAULT_CHANNEL_LOG_STREAM_NAME: &str = "DestinationDelivery";

/// The key prefix the dead-letter queue a general purpose destination gets by default
/// writes error records under: that queue "defaults to the destination bucket with an error
/// prefix" (`S3DestinationConfiguration$DeadLetterQueueS3Configuration`), which names the
/// bucket and the prefix's purpose but no text, so this prefix is CHOSEN, inside the
/// modelled `S3ErrorOutputPrefix` pattern.
pub const DEFAULT_ERROR_OUTPUT_PREFIX: &str = "errors/";

/// The alias of the KMS key this service owns, which a channel cannot be encrypted with:
/// "You cannot use the Amazon Kinesis Data Streams service key (aws/kinesis)"
/// (`ChannelEncryptionConfiguration$KeyId`).
pub const KINESIS_SERVICE_KEY_ALIAS: &str = "alias/aws/kinesis";

/// The prefix of the `alias/<name>` form a KMS key identifier may take: "Alias name
/// example: alias/MyAliasName" (`StartStreamEncryptionInput$KeyId`).
const KMS_ALIAS_PREFIX: &str = "alias/";

/// The prefix of the `key/<id>` resource an ARN-form KMS key identifier names: "Key ARN
/// example: arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012"
/// (`StartStreamEncryptionInput$KeyId`).
const KMS_KEY_RESOURCE_PREFIX: &str = "key/";

/// What every ARN starts with, which is what tells an ARN-form identifier from a bare one.
const ARN_PREFIX: &str = "arn:";

/// How many channels one region's endpoint holds. The emulator's OWN ceiling, not a
/// modelled limit: the store grows by one record per accepted create, so the creates one
/// caller can make are bounded.
pub const MAX_CHANNELS_PER_REGION: usize = 50;

/// How many channels this emulator holds across every region — the ceiling no request can
/// partition, since the region a create is counted against is the caller's to choose. The
/// emulator's OWN ceiling, not a modelled limit.
pub const MAX_CHANNELS_TOTAL: usize = 100;

/// How many channels `ListChannels` returns when the request names no `MaxResults`: "The
/// maximum number of channels to return in a single call. The default value is 100"
/// (`ListChannelsInput$MaxResults`).
pub const DEFAULT_LIST_CHANNELS_LIMIT: i32 = 100;

/// The most channels `ListChannels` returns in one call: "If you specify a value greater
/// than 100, at most 100 results are returned" (`ListChannelsInput$MaxResults`), which the
/// modelled `range 1..10000` deliberately leaves room above.
pub const MAX_LIST_CHANNELS_LIMIT: i32 = 100;

/// How many seconds an hour of a retention window holds, so the window a stream reports in
/// hours can be measured against stored instants.
const SECONDS_PER_HOUR: u64 = 3_600;

/// How a stream is keyed: the region its creating request was signed for, and the name
/// that request carried.
///
/// "The stream name identifies the stream. The name is scoped to the Amazon Web Services
/// account used by the application. It is also scoped by Amazon Web Services Region"
/// (`CreateStream`) — so the same name in two regions is two streams, and only the region
/// that created one resolves it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegionalName {
    pub region: String,
    pub name: String,
}

impl RegionalName {
    /// The key `name` occupies in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }
}

/// One data record of a shard — "the unit of data of the Kinesis data stream, which is
/// composed of a sequence number, a partition key, and a data blob" (`Record`) — as a
/// write stored it and a read answers it.
#[derive(Debug, Clone)]
pub struct DataRecord {
    /// `Record$SequenceNumber` — "The unique identifier of the record within its shard",
    /// as [`record_sequence_number`] mints it.
    pub sequence_number: String,
    /// `Record$ApproximateArrivalTimestamp` — "The approximate time that the record was
    /// inserted into the stream", which is also what a `AT_TIMESTAMP` reader positions by
    /// and what the retention window is measured against.
    pub arrived_at: SystemTime,
    /// `Record$PartitionKey` — "Identifies which shard in the stream the data record is
    /// assigned to". `None` for a record written to an `AUTO` stream without one, which is
    /// the one case the member documents as absent.
    pub partition_key: Option<String>,
    /// `Record$Data` — "The data blob. The data in the blob is both opaque and immutable
    /// to Kinesis Data Streams, which does not inspect, interpret, or change the data in
    /// the blob in any way", so it is stored and answered byte for byte.
    pub data: Vec<u8>,
    /// `Record$EncryptionType` — "The encryption type used on the record", which is the
    /// stream's own at the instant the record was written.
    pub encryption_type: String,
}

impl DataRecord {
    /// This record as the modelled `Record` shape.
    pub fn to_shape(&self) -> Record {
        Record {
            sequence_number: self.sequence_number.clone(),
            approximate_arrival_timestamp: Some(timestamp(self.arrived_at)),
            data: self.data.clone(),
            partition_key: self.partition_key.clone(),
            encryption_type: Some(self.encryption_type.clone()),
        }
    }

    /// The bytes the record occupies in the store, which is what
    /// [`MAX_RECORD_BYTES_PER_SHARD`] bounds and what one `GetRecords` page is measured
    /// against ([`MAX_GET_RECORDS_BYTES`]).
    ///
    /// Both halves of the record count, the same way a write measures what it may store:
    /// "When the data blob (the payload before base64-encoding) is added to the partition
    /// key size, the total size must not exceed the maximum record size"
    /// (`PutRecordInput$Data`).
    pub fn stored_bytes(&self) -> usize {
        self.data.len()
            + self
                .partition_key
                .as_deref()
                .map(str::len)
                .unwrap_or_default()
    }
}

/// One shard of a stream's shard map: the slice of the hash key space it covers, the
/// sequence numbers it spans, and the data records written to it.
#[derive(Debug, Clone)]
pub struct ShardRecord {
    /// The shard's id, as [`shard_id`] mints it.
    pub shard_id: String,
    /// Decimal `StartingHashKey`, inside the `HashKey` pattern `^(0|([1-9]\d{0,38}))$`.
    pub starting_hash_key: String,
    /// Decimal `EndingHashKey`.
    pub ending_hash_key: String,
    /// The first sequence number the shard can hold.
    pub starting_sequence_number: String,
    /// The last one it can hold. `None` while the shard is open — "Shards that are in the
    /// OPEN state have an ending sequence number of null"
    /// (`SequenceNumberRange$EndingSequenceNumber`).
    pub ending_sequence_number: Option<String>,
    /// When the shard was opened: the instant the create or the reshard that laid it down
    /// ran. A shard filter selects by it, so it is stored rather than derived.
    pub created_at: SystemTime,
    /// When the shard was closed, `None` while it is open. A reshard is what closes a
    /// shard, and it sets this instant and [`Self::ending_sequence_number`] together.
    pub closed_at: Option<SystemTime>,
    /// `Shard$ParentShardId` — "The shard ID of the shard's parent": the shard this one
    /// was produced from by a split or a merge. `None` for a shard a create laid down.
    pub parent_shard_id: Option<String>,
    /// `Shard$AdjacentParentShardId` — "The shard ID of the shard adjacent to the shard's
    /// parent": the second shard this one was produced from, which only a merge has.
    pub adjacent_parent_shard_id: Option<String>,
    /// The records written to the shard, oldest first — the order `GetRecords` reads them
    /// in ("the shard iterator specifies the position in the shard from which you want to
    /// start reading data records sequentially").
    pub records: Vec<DataRecord>,
    /// How many records have ever been written to the shard, which is what the next
    /// sequence number is minted from ([`record_sequence_number`]). Counted rather than
    /// taken from the stored records, so a number a trim or the store's ceiling released
    /// is never handed out twice.
    pub records_written: u64,
    /// How many bytes [`Self::records`] occupies, maintained as records are appended and
    /// dropped so the write path never re-sums the whole shard.
    pub records_bytes: usize,
    /// What the shard has left of the write rate it admits records at — "each shard can
    /// support writes up to 1,000 records per second, up to a maximum data write total of 1
    /// MB per second" (`PutRecords`). Metered PER SHARD, so it is held here rather than on
    /// the stream ([`WriteQuota`]).
    write_quota: WriteQuota,
}

impl ShardRecord {
    /// A brand-new OPEN shard covering `starting_hash_key`..`ending_hash_key`, holding no
    /// records: what a create lays down.
    pub fn open_at(
        index: u32,
        created_at: SystemTime,
        starting_hash_key: String,
        ending_hash_key: String,
    ) -> Self {
        Self {
            shard_id: shard_id(index),
            starting_hash_key,
            ending_hash_key,
            starting_sequence_number: starting_sequence_number(created_at, index),
            // A shard a create laid down is OPEN, so it has no ending sequence number and
            // no close instant.
            ending_sequence_number: None,
            created_at,
            closed_at: None,
            // A shard a create laid down came from no split and no merge, so it names no
            // parent; [`Self::from_parents`] is what a reshard lays a child down with.
            parent_shard_id: None,
            adjacent_parent_shard_id: None,
            records: Vec::new(),
            records_written: 0,
            records_bytes: 0,
            // A shard nothing has been written to admits a second's worth of writes at
            // once. The rate is measured against the monotonic clock, which is the clock a
            // rate has to be read on: a wall-clock jump must not hand out capacity.
            write_quota: WriteQuota::fresh(Instant::now()),
        }
    }

    /// The same brand-new OPEN shard, produced from the shards named in `parents` — the
    /// open shards whose hash key ranges the new one covers.
    ///
    /// "The shard ID of the shard's parent" and "the shard ID of the shard adjacent to the
    /// shard's parent" (`Shard`): a child of ONE shard is a split's, a child of two is a
    /// merge's. A re-tiling that covers more than two of them reaches the child through
    /// short-lived intermediate shards — "this can cause short-lived shards to be created,
    /// in addition to the final shards" (`UpdateShardCount`) — none of which this emulator
    /// lays down, so the lineage it records names the lowest and the highest of the shards
    /// the child covers.
    pub fn from_parents(
        index: u32,
        created_at: SystemTime,
        starting_hash_key: String,
        ending_hash_key: String,
        parents: &[String],
    ) -> Self {
        Self {
            parent_shard_id: parents.first().cloned(),
            // One parent is a split's child: it has a parent and no adjacent parent.
            adjacent_parent_shard_id: parents.last().filter(|_| parents.len() > 1).cloned(),
            ..Self::open_at(index, created_at, starting_hash_key, ending_hash_key)
        }
    }

    /// Close the shard at `at`, which is what a reshard does to every shard it re-tiles:
    /// the shard stops being open, so it takes the ending sequence number its write
    /// counter has reached — "shards that are in the OPEN state have an ending sequence
    /// number of null" (`SequenceNumberRange$EndingSequenceNumber`), and a closed one does
    /// not. The records it holds stay readable until the retention window releases them.
    pub fn close_at(&mut self, at: SystemTime) {
        self.ending_sequence_number = Some(record_sequence_number(
            &self.starting_sequence_number,
            self.records_written,
        ));
        self.closed_at = Some(at);
    }

    /// Whether `shard_id` is one of the shards this one was produced from.
    pub fn is_child_of(&self, shard_id: &str) -> bool {
        [
            self.parent_shard_id.as_deref(),
            self.adjacent_parent_shard_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|parent| parent == shard_id)
    }

    /// The shards this one was produced from, as `ChildShard$ParentShards` reports them.
    pub fn parent_shard_ids(&self) -> Vec<String> {
        [
            self.parent_shard_id.clone(),
            self.adjacent_parent_shard_id.clone(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// The slice of the hash key space this shard covers, as a pair of integers. A bound
    /// this emulator did not write — one that does not parse — covers nothing.
    pub fn hash_key_bounds(&self) -> Option<(u128, u128)> {
        Some((
            self.starting_hash_key.parse::<u128>().ok()?,
            self.ending_hash_key.parse::<u128>().ok()?,
        ))
    }

    /// Whether this shard's write quota admits a record occupying `bytes` at `at`, charging
    /// the quota for it when it does ([`WriteQuota::admits`]). `max_record_bytes` is what one
    /// record of the stream this shard belongs to may carry, which is what the shard's write
    /// capacity is read from.
    ///
    /// Consulted by every write before it appends, and by nothing else: a READ of a shard
    /// spends no write capacity.
    pub fn admits_write(&mut self, bytes: usize, max_record_bytes: usize, at: Instant) -> bool {
        self.write_quota.admits(bytes, max_record_bytes, at)
    }

    /// Append `record` to the shard and answer the sequence number it was stored under.
    ///
    /// The number comes from the shard's own write counter, so it is unique within the
    /// shard and orders the records the way they were written — which is the order a
    /// reader reads them in.
    pub fn append(
        &mut self,
        partition_key: Option<String>,
        data: Vec<u8>,
        encryption_type: &str,
        arrived_at: SystemTime,
    ) -> String {
        let sequence_number =
            record_sequence_number(&self.starting_sequence_number, self.records_written);
        self.records_written = self.records_written.saturating_add(1);
        let record = DataRecord {
            sequence_number: sequence_number.clone(),
            arrived_at,
            partition_key,
            data,
            encryption_type: encryption_type.to_string(),
        };
        self.records_bytes = self.records_bytes.saturating_add(record.stored_bytes());
        self.records.push(record);
        self.enforce_storage_ceiling();
        sequence_number
    }

    /// Hold the shard inside the emulator's own ceilings ([`MAX_RECORDS_PER_SHARD`] /
    /// [`MAX_RECORD_BYTES_PER_SHARD`]) by dropping its OLDEST records, which is the end a
    /// retention window releases too.
    ///
    /// The NEWEST record is never dropped: a write answers the sequence number it stored a
    /// record under, so that record has to be there for the reader that is handed it.
    fn enforce_storage_ceiling(&mut self) {
        let droppable = self.records.len().saturating_sub(1);
        let mut dropped = 0usize;
        while dropped < droppable {
            if self.records.len() - dropped <= MAX_RECORDS_PER_SHARD
                && self.records_bytes <= MAX_RECORD_BYTES_PER_SHARD
            {
                break;
            }
            let Some(record) = self.records.get(dropped) else {
                break;
            };
            self.records_bytes = self.records_bytes.saturating_sub(record.stored_bytes());
            dropped += 1;
        }
        if dropped > 0 {
            self.records.drain(..dropped);
            // The emulator's own ceiling, not the stream's retention window, released these
            // records: a reader sees a shorter shard than the window would have kept, so the
            // difference is diagnosable rather than invisible.
            ::tracing::debug!(
                shard_id = %self.shard_id,
                dropped,
                retained = self.records.len(),
                records_bytes = self.records_bytes,
                "dropped the shard's oldest records at the emulator's per-shard ceiling"
            );
        }
    }

    /// The records the shard still retains at `trim_horizon`: "a stream's data records are
    /// accessible for 24 hours from the time they are added to the stream" and for the
    /// stream's `RetentionPeriodHours` once that window is changed, so a record that
    /// arrived before the horizon is trimmed rather than read.
    pub fn retained(&self, trim_horizon: SystemTime) -> impl DoubleEndedIterator<Item = &DataRecord> {
        self.records
            .iter()
            .filter(move |record| record.arrived_at >= trim_horizon)
    }

    /// This record as the modelled `Shard` shape.
    pub fn to_shape(&self) -> Shard {
        Shard {
            shard_id: self.shard_id.clone(),
            // Both are set only on a shard produced by a split or a merge, which is what a
            // reshard performs ([`Self::from_parents`]); a shard a create laid down has
            // neither.
            parent_shard_id: self.parent_shard_id.clone(),
            adjacent_parent_shard_id: self.adjacent_parent_shard_id.clone(),
            hash_key_range: HashKeyRange {
                starting_hash_key: self.starting_hash_key.clone(),
                ending_hash_key: self.ending_hash_key.clone(),
            },
            sequence_number_range: SequenceNumberRange {
                starting_sequence_number: self.starting_sequence_number.clone(),
                ending_sequence_number: self.ending_sequence_number.clone(),
            },
        }
    }

    /// Whether the shard is still open, so it counts towards `OpenShardCount`.
    pub fn is_open(&self) -> bool {
        self.ending_sequence_number.is_none() && self.closed_at.is_none()
    }

    /// Whether the shard was open at `instant`: it had been laid down by then and nothing
    /// had closed it yet.
    pub fn was_open_at(&self, instant: SystemTime) -> bool {
        self.created_at <= instant && self.closed_at.is_none_or(|closed| closed > instant)
    }

    /// Whether the shard still held data at `instant`: it was laid down no later than
    /// `instant` and, if it has been closed, closed no earlier.
    pub fn spans(&self, instant: SystemTime) -> bool {
        self.created_at <= instant && self.closed_at.is_none_or(|closed| closed >= instant)
    }

    /// Whether the shard is open or was closed no earlier than `instant`.
    pub fn open_or_closed_since(&self, instant: SystemTime) -> bool {
        self.closed_at.is_none_or(|closed| closed >= instant)
    }

    /// Whether `hash_key` falls inside the slice of the hash key space this shard covers
    /// (`Shard$HashKeyRange`, "The range of possible hash key values for the shard, which
    /// is a set of ordered contiguous positive integers"). A range this emulator did not
    /// write — one whose bounds do not parse — covers nothing rather than everything.
    pub fn covers(&self, hash_key: u128) -> bool {
        let Ok(starting) = self.starting_hash_key.parse::<u128>() else {
            return false;
        };
        let Ok(ending) = self.ending_hash_key.parse::<u128>() else {
            return false;
        };
        (starting..=ending).contains(&hash_key)
    }
}

/// One consumer registered with a stream for enhanced fan-out: the authoritative record
/// `RegisterStreamConsumer` writes and `DescribeStreamConsumer` projects onto its output
/// shape.
#[derive(Debug, Clone)]
pub struct ConsumerRecord {
    /// The `ConsumerName` the registration carried, which is what names this consumer
    /// inside its stream: "For a given Kinesis data stream, each consumer must have a
    /// unique name. However, consumer names don't have to be unique across data streams"
    /// (`RegisterStreamConsumerInput$ConsumerName`).
    pub name: String,
    /// The `ConsumerARN`, as [`consumer_arn`] builds it.
    pub arn: String,
    /// When `RegisterStreamConsumer` minted the consumer.
    pub created_at: SystemTime,
    /// When the consumer's `CREATING` -> `ACTIVE` transition falls due: a registered
    /// consumer "can't read data while in the CREATING or DELETING states"
    /// (`Consumer$ConsumerStatus`), so the status it starts in is not terminal.
    pub active_at: SystemTime,
    /// The tags the registration carried: "Tags will take effect from the CREATING status
    /// of the consumer" (`RegisterStreamConsumer`).
    pub tags: HashMap<String, String>,
    /// The resource-based policy attached to this consumer, `None` while none is
    /// ([`ComAmazonawsKinesisKinesis20131202State::resource_policy_by_arn`]): a policy is
    /// "attached to the specified data stream or consumer" (`GetResourcePolicy`), so the
    /// consumer it governs is where it is stored, and it is gone when the consumer is.
    pub policy: Option<String>,
}

impl ConsumerRecord {
    /// The consumer a registration at `at` writes for `name` under the stream `stream_arn`
    /// addresses.
    pub fn registered_at(
        stream_arn: &str,
        name: &str,
        at: SystemTime,
        tags: HashMap<String, String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn: consumer_arn(stream_arn, name, at),
            created_at: at,
            // CREATING is not terminal: the consumer is owed ACTIVE, and the instant that
            // is due is stored rather than slept on.
            active_at: at + CONSUMER_ACTIVATION_DELAY,
            tags,
            // A resource-based policy is `PutResourcePolicy`'s to attach, so a consumer a
            // registration has just minted carries none.
            policy: None,
        }
    }

    /// The consumer's status at `now`: `CREATING` until the transition its registration
    /// armed falls due, `ACTIVE` from then on. Derived from the stored instant, so two
    /// reads at the same instant agree and no read has to write.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.active_at {
            CONSUMER_CREATING
        } else {
            CONSUMER_ACTIVE
        }
    }

    /// This record as the modelled `Consumer`, which is what a registration answers with.
    pub fn to_shape(&self, now: SystemTime) -> Consumer {
        Consumer {
            consumer_name: self.name.clone(),
            consumer_arn: self.arn.clone(),
            consumer_status: self.status(now).to_string(),
            consumer_creation_timestamp: timestamp(self.created_at),
        }
    }

    /// This record as the modelled `ConsumerDescription`, which carries the ARN of "the
    /// stream with which you registered the consumer"
    /// (`ConsumerDescription$StreamARN`) beside everything the `Consumer` shape holds.
    pub fn to_description(&self, now: SystemTime, stream_arn: &str) -> ConsumerDescription {
        ConsumerDescription {
            consumer_name: self.name.clone(),
            consumer_arn: self.arn.clone(),
            consumer_status: self.status(now).to_string(),
            consumer_creation_timestamp: timestamp(self.created_at),
            stream_arn: stream_arn.to_string(),
        }
    }
}

/// One stored Kinesis data stream: the authoritative record every stream operation reads.
/// `CreateStream` inserts it and `DescribeStream` / `DescribeStreamSummary` project it
/// onto their output shapes.
#[derive(Debug, Clone)]
pub struct Stream {
    /// The `StreamName` the create carried, which is half of this record's key.
    pub name: String,
    /// The region the creating request was signed for.
    pub region: String,
    /// The `StreamARN`, as [`stream_arn`] builds it.
    pub arn: String,
    /// `StreamMode` enum value — `PROVISIONED` or `ON_DEMAND`.
    pub stream_mode: String,
    /// The stream's shard map, lowest hash key first.
    pub shards: Vec<ShardRecord>,
    /// `RetentionPeriodHours`.
    pub retention_period_hours: i32,
    /// When `CreateStream` minted the stream.
    pub created_at: SystemTime,
    /// When the stream's `CREATING` -> `ACTIVE` transition falls due: "Upon receiving a
    /// CreateStream request, Kinesis Data Streams immediately returns and sets the stream
    /// status to CREATING. After the stream is created, Kinesis Data Streams sets the
    /// stream status to ACTIVE" (`CreateStream`).
    pub active_at: SystemTime,
    /// `EncryptionType` enum value.
    pub encryption_type: String,
    /// The KMS key encryption uses, set only once a stream is encrypted.
    pub key_id: Option<String>,
    /// The shard-level metrics enhanced monitoring reports. Empty until
    /// `EnableEnhancedMonitoring` names some.
    pub shard_level_metrics: Vec<String>,
    /// `WarmThroughputMiBps`, when a create or an update asked for a target warm throughput.
    pub warm_throughput_mi_bps: Option<i32>,
    /// `MaxRecordSizeInKiB`: the maximum size of one record this stream is configured for.
    /// Every stream carries one — the value its create or a later update named, else
    /// [`DEFAULT_MAX_RECORD_SIZE_KI_B`] — so the value a read reports is the value every
    /// write to the stream is measured against ([`Stream::max_record_size_bytes`]).
    pub max_record_size_in_ki_b: i32,
    /// `RecordDistributionStrategy`, which only an on-demand stream carries.
    pub record_distribution_strategy: Option<String>,
    /// When the stream's `DELETING` -> gone transition falls due, set by the
    /// `DeleteStream` that accepted the removal: "After a DeleteStream request, the
    /// specified stream is in the DELETING state until Kinesis Data Streams completes the
    /// deletion" (`DeleteStream`). `None` for a stream nothing has deleted.
    pub deleted_at: Option<SystemTime>,
    /// When the stream's `UPDATING` -> `ACTIVE` transition falls due, set by the accepted
    /// change that armed it — a shard-count re-tiling, a capacity-mode change or a
    /// warm-throughput scaling: "Upon receiving the request... sets the status of the stream
    /// to UPDATING. After the update is complete, Kinesis Data Streams sets the status of the
    /// stream back to ACTIVE" (`UpdateShardCount`, `UpdateStreamWarmThroughput`). `None` for
    /// a stream nothing has changed.
    pub updating_until: Option<SystemTime>,
    /// The instants the stream's shard count was changed at, youngest last — what the
    /// documented "ten times per rolling 24-hour period per stream" ceiling
    /// ([`MAX_RESHARDS_PER_WINDOW`]) is counted over. Pruned to [`RESHARD_WINDOW`] by the
    /// change that counts it, so it holds at most that many instants.
    pub reshards: Vec<SystemTime>,
    /// The instants a KMS key was applied to the stream at, youngest last — what the
    /// documented "25 times in a rolling 24-hour period" ceiling
    /// ([`MAX_ENCRYPTION_CHANGES_PER_WINDOW`]) is counted over for enabling encryption.
    /// Only a change that SUCCEEDED is recorded, since that is what the limit counts, and
    /// the list is pruned to [`ENCRYPTION_CHANGE_WINDOW`] by the change that counts it.
    pub encryption_starts: Vec<SystemTime>,
    /// The instants server-side encryption was disabled on the stream at, counted against
    /// the same ceiling for disabling it and kept the same way as
    /// [`Self::encryption_starts`].
    pub encryption_stops: Vec<SystemTime>,
    /// The tags the create carried.
    pub tags: HashMap<String, String>,
    /// The consumers registered with the stream for enhanced fan-out, in registration
    /// order. Empty until `RegisterStreamConsumer` registers one, and gone with the stream
    /// once its removal falls due: "When you delete a stream, any shards in that stream are
    /// also deleted" (`DeleteStream`), and a consumer is registered with the stream.
    pub consumers: Vec<ConsumerRecord>,
    /// The resource-based policy attached to the stream itself, `None` while none is: a
    /// policy is "attached to the specified data stream or consumer" (`GetResourcePolicy`),
    /// so the stream it governs is where it is stored, and it is gone when the stream is.
    pub policy: Option<String>,
}

impl Stream {
    /// The strategy that places a record written to this stream: the one the create named,
    /// or the documented default — "If the stream uses the USER_PARTITION_KEY record
    /// distribution strategy (the default), a partition key is required"
    /// (`PutRecordInput$PartitionKey`).
    pub fn record_distribution(&self) -> &str {
        self.record_distribution_strategy
            .as_deref()
            .unwrap_or(RECORD_DISTRIBUTION_USER_PARTITION_KEY)
    }

    /// `RecordDistributionStrategy` as a READ of the stream reports it: the strategy the
    /// stream places its records by ([`Self::record_distribution`]) for a stream whose
    /// capacity mode carries the setting at all, and nothing for one whose mode does not.
    ///
    /// "This field is only present for streams that use the on-demand capacity mode"
    /// (`StreamDescriptionSummary$RecordDistributionStrategy`), so the member is reported
    /// exactly for an on-demand stream — with the documented default for one nothing has set
    /// a strategy on — and omitted for a provisioned stream, which "does not support the
    /// record distribution strategy setting" (`UpdateStreamRecordDistributionStrategy`).
    ///
    /// Derived from the stored mode and strategy rather than stored a second time, so a
    /// stream that changes capacity mode cannot report a setting its mode does not have.
    pub fn reported_record_distribution(&self) -> Option<String> {
        (self.stream_mode == STREAM_MODE_ON_DEMAND)
            .then(|| self.record_distribution().to_string())
    }

    /// Which shard of the map a record carrying `explicit_hash_key` / `partition_key` is
    /// assigned to, by the index it occupies in [`Self::shards`].
    ///
    /// "Amazon Kinesis Data Streams uses the partition key as input to a hash function that
    /// maps the partition key and associated data to a specific shard... all data records
    /// with the same partition key map to the same shard within the stream"
    /// (`PutRecordInput$PartitionKey`), and `ExplicitHashKey` names that hash value
    /// directly ("The hash value used to explicitly determine the shard the data record is
    /// assigned to"). Under the `AUTO` strategy "the partition key is optional and any
    /// value you provide is ignored, along with any ExplicitHashKey you provide... Amazon
    /// Kinesis Data Streams distributes the record across shards using service-managed
    /// algorithms", so the record goes to the open shard the stream's write count selects,
    /// advanced by `nth_in_call` — the record's position within the call being served — so
    /// that one batch spreads over every open shard rather than landing in one.
    pub fn placement(
        &self,
        explicit_hash_key: Option<&str>,
        partition_key: Option<&str>,
        nth_in_call: u64,
    ) -> Result<usize, RecordPlacementFailure> {
        let open: Vec<usize> = self
            .shards
            .iter()
            .enumerate()
            .filter(|(_, shard)| shard.is_open())
            .map(|(index, _)| index)
            .collect();
        if open.is_empty() {
            return Err(RecordPlacementFailure::NoOpenShard);
        }
        if self.record_distribution() == RECORD_DISTRIBUTION_AUTO {
            let written: u64 = self
                .shards
                .iter()
                .fold(0u64, |total, shard| total.saturating_add(shard.records_written));
            let position = usize::try_from(written.wrapping_add(nth_in_call) % open.len() as u64)
                .unwrap_or(0);
            return open
                .get(position)
                .copied()
                .ok_or(RecordPlacementFailure::NoOpenShard);
        }
        let hash = match explicit_hash_key {
            // `ExplicitHashKey` is modelled `^(0|([1-9]\d{0,38}))$`, which admits values
            // past `u128::MAX`: such a value is outside the key space a hash key lives in,
            // which the service range-checks itself.
            Some(explicit) => explicit
                .parse::<u128>()
                .map_err(|_| RecordPlacementFailure::HashKeyOutOfRange(explicit.to_string()))?,
            None => {
                let Some(partition_key) = partition_key else {
                    return Err(RecordPlacementFailure::PartitionKeyRequired);
                };
                partition_key_hash(partition_key)
            }
        };
        open.into_iter()
            .find(|index| {
                self.shards
                    .get(*index)
                    .is_some_and(|shard| shard.covers(hash))
            })
            .ok_or_else(|| RecordPlacementFailure::OutsideTheShardMap(hash.to_string()))
    }

    /// The most bytes one record written to this stream may carry: the stream's own
    /// `MaxRecordSizeInKiB`, which is the value a read of the stream reports — "Each
    /// data record can be up to 1 MiB in size by default. Amazon Kinesis Data Streams
    /// supports large records up to 10 MiB in size" (`GetRecords`), the larger sizes being
    /// what `MaxRecordSizeInKiB` opts into.
    pub fn max_record_size_bytes(&self) -> usize {
        let ki_b = self.max_record_size_in_ki_b.max(1);
        usize::try_from(ki_b).unwrap_or(usize::MAX).saturating_mul(1024)
    }

    /// Whether the shard at `index` in the shard map — the index [`Self::placement`]
    /// resolved — admits a record occupying `bytes` at `at`, charging that shard's write
    /// quota when it does, together with the shard's id, which the refusal names.
    /// `None` for an index the map does not hold.
    ///
    /// ONE path for every write, exactly as [`Self::append_at`] is: `PutRecord` and
    /// `PutRecords` meter a record against the shard it is placed in by this one rule, so a
    /// producer cannot pass the shard's rate by splitting its writes over the two
    /// operations.
    pub fn charge_write_at(
        &mut self,
        index: usize,
        bytes: usize,
        at: Instant,
    ) -> Option<(bool, String)> {
        let max_record_bytes = self.max_record_size_bytes();
        let shard = self.shards.get_mut(index)?;
        let admitted = shard.admits_write(bytes, max_record_bytes, at);
        Some((admitted, shard.shard_id.clone()))
    }

    /// The KMS key the stream's records are encrypted under, and `None` for a stream that is
    /// not encrypted at all.
    ///
    /// What the data plane asks KMS about on every call: "KMS: Use server-side encryption on
    /// the records in the stream using a customer-managed Amazon Web Services KMS key"
    /// (`PutRecordOutput$EncryptionType`), so the key's own state decides whether those
    /// records can be written and read at all.
    pub fn encryption_key(&self) -> Option<&str> {
        self.key_id
            .as_deref()
            .filter(|_| self.encryption_type == ENCRYPTION_TYPE_KMS)
    }

    /// Append one record to the shard at `index` in the shard map — the index
    /// [`Self::placement`] resolved for it — answering that shard's id and the sequence
    /// number the record was stored under, or `None` for an index the map does not hold.
    ///
    /// ONE path for every write: `PutRecord` and `PutRecords` resolve the shard by the same
    /// rule ([`Self::placement`]) and store through this append, so a reader cannot tell
    /// which operation wrote a record.
    ///
    /// The partition key is stored as the producer sent it: under the `AUTO` strategy "the
    /// partition key is optional and any value you provide is ignored", but
    /// `Record$PartitionKey` is still "the original value... even though it was not used to
    /// determine shard placement".
    pub fn append_at(
        &mut self,
        index: usize,
        partition_key: Option<String>,
        data: Vec<u8>,
        at: SystemTime,
    ) -> Option<(String, String)> {
        let encryption_type = self.encryption_type.clone();
        let shard = self.shards.get_mut(index)?;
        let sequence_number = shard.append(partition_key, data, &encryption_type, at);
        Some((shard.shard_id.clone(), sequence_number))
    }

    /// The stream's status at `now`: `DELETING` once a delete has been accepted for it,
    /// `CREATING` until the transition the create armed falls due, `UPDATING` until the
    /// one an accepted change armed falls due, `ACTIVE` from then on. Derived from the
    /// stored instants, so two reads at the same instant agree and no read has to write.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.deleted_at.is_some() {
            STREAM_DELETING
        } else if now < self.active_at {
            STREAM_CREATING
        } else if self.updating_until.is_some_and(|due| now < due) {
            STREAM_UPDATING
        } else {
            STREAM_ACTIVE
        }
    }

    /// Whether the stream serves its shard map and its records at `now`.
    ///
    /// "You can continue to read and write data to your stream while its status is
    /// UPDATING" (`UpdateShardCount`), so a stream whose re-tiling is still due serves a
    /// read and a write exactly as an `ACTIVE` stream does; a stream whose create or whose
    /// removal is still due has no shard map to serve at all.
    pub fn serves_shards(&self, now: SystemTime) -> bool {
        matches!(self.status(now), STREAM_ACTIVE | STREAM_UPDATING)
    }

    /// Whether records written to the stream are encrypted by the service, which is what
    /// disabling encryption needs there to be something to disable.
    pub fn is_encrypted(&self) -> bool {
        self.encryption_type == ENCRYPTION_TYPE_KMS
    }

    /// How many times a KMS key has been applied to the stream inside the window ending at
    /// `now`, which is what [`MAX_ENCRYPTION_CHANGES_PER_WINDOW`] bounds for enabling
    /// encryption.
    pub fn encryption_starts_in_window(&self, now: SystemTime) -> usize {
        encryption_changes_in_window(&self.encryption_starts, now)
    }

    /// How many times encryption has been disabled on the stream inside the window ending
    /// at `now`, which is what the same ceiling bounds for disabling it.
    pub fn encryption_stops_in_window(&self, now: SystemTime) -> usize {
        encryption_changes_in_window(&self.encryption_stops, now)
    }

    /// Turn server-side encryption ON at `at` with `key_id`, which is also how the key an
    /// encrypted stream already uses is UPDATED: "Enables or updates server-side encryption
    /// using an Amazon Web Services KMS key for a specified stream"
    /// (`StartStreamEncryption`).
    ///
    /// "Once the status of the stream is ACTIVE, encryption begins for records written to
    /// the stream", and UPDATING is not terminal: the instant the stream is owed `ACTIVE`
    /// at is stored rather than slept on. The change counts towards the rolling-window
    /// ceiling.
    pub fn start_encryption(&mut self, key_id: &str, at: SystemTime) {
        self.encryption_type = ENCRYPTION_TYPE_KMS.to_string();
        self.key_id = Some(key_id.to_string());
        self.updating_until = Some(at + ENCRYPTION_UPDATE_DELAY);
        record_encryption_change(&mut self.encryption_starts, at);
    }

    /// Turn server-side encryption OFF at `at`: the stream stops naming a KMS key, so
    /// records written to it from here on are no longer encrypted by the service.
    ///
    /// "Upon receiving the request, Kinesis Data Streams returns immediately and sets the
    /// status of the stream to UPDATING. After the update is complete, Kinesis Data Streams
    /// sets the status of the stream back to ACTIVE" (`StopStreamEncryption`), so UPDATING
    /// is not terminal: the instant the stream is owed `ACTIVE` at is stored rather than
    /// slept on. The disable counts towards the rolling-window ceiling.
    pub fn stop_encryption(&mut self, at: SystemTime) {
        self.encryption_type = ENCRYPTION_TYPE_NONE.to_string();
        self.key_id = None;
        self.updating_until = Some(at + ENCRYPTION_UPDATE_DELAY);
        record_encryption_change(&mut self.encryption_stops, at);
    }

    /// How many shard-count changes the stream has been scaled by inside the window
    /// ending at `now`, which is what [`MAX_RESHARDS_PER_WINDOW`] bounds.
    pub fn reshards_in_window(&self, now: SystemTime) -> usize {
        let from = now.checked_sub(RESHARD_WINDOW).unwrap_or(UNIX_EPOCH);
        self.reshards.iter().filter(|at| **at >= from).count()
    }

    /// Re-tile the stream's open shards onto `target_shard_count` equal slices of the hash
    /// key space at `at`, answering how many shards were open before.
    ///
    /// "To update the shard count, Kinesis Data Streams performs splits or merges on
    /// individual shards" (`UpdateShardCount`), and a uniform scaling "creates shards of
    /// equal size" (`ScalingType`): every shard that was open is closed, and the whole key
    /// space is covered again by `target_shard_count` contiguous equal slices, each child
    /// naming the shards it was produced from. A shard that is already closed is left as it
    /// is, so the records it holds stay readable until the retention window releases them,
    /// and a map that already tiles the key space the target way is left untouched —
    /// nothing is split or merged to reach a tiling it already has.
    ///
    /// Bounded by the caller: a target past the shard ceilings is refused BEFORE this
    /// allocates from it.
    pub fn reshard_uniformly(&mut self, target_shard_count: u32, at: SystemTime) -> i32 {
        let open_before = self.open_shard_count();
        let target = hash_key_ranges(target_shard_count);
        let parents: Vec<(u128, u128, String)> = self
            .shards
            .iter()
            .filter(|shard| shard.is_open())
            .filter_map(|shard| {
                let (starting, ending) = shard.hash_key_bounds()?;
                Some((starting, ending, shard.shard_id.clone()))
            })
            .collect();

        // The shard count is being changed, so the stream owes the `ACTIVE` it goes back
        // to, and this change counts towards the rolling-window ceiling.
        self.updating_until = Some(at + RESHARD_DELAY);
        let from = at.checked_sub(RESHARD_WINDOW).unwrap_or(UNIX_EPOCH);
        self.reshards.retain(|instant| *instant >= from);
        self.reshards.push(at);

        // A map whose open shards already are the target tiling needs no split and no
        // merge: the shards stay open, keeping the sequence numbers their readers hold.
        let unchanged = parents.len() == target.len()
            && target.iter().zip(parents.iter()).all(
                |((starting, ending), (parent_starting, parent_ending, _))| {
                    starting.parse::<u128>().ok() == Some(*parent_starting)
                        && ending.parse::<u128>().ok() == Some(*parent_ending)
                },
            );
        if unchanged {
            return open_before;
        }

        for shard in self.shards.iter_mut().filter(|shard| shard.is_open()) {
            shard.close_at(at);
        }
        // A shard id is the shard's position in the map, so the children take the
        // positions after every shard the map already holds and no id is ever reused.
        let first_index = u32::try_from(self.shards.len()).unwrap_or(u32::MAX);
        for (offset, (starting_hash_key, ending_hash_key)) in target.into_iter().enumerate() {
            let index = first_index.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            let covered: Vec<String> = match (
                starting_hash_key.parse::<u128>(),
                ending_hash_key.parse::<u128>(),
            ) {
                (Ok(starting), Ok(ending)) => parents
                    .iter()
                    .filter(|(parent_starting, parent_ending, _)| {
                        *parent_starting <= ending && *parent_ending >= starting
                    })
                    .map(|(_, _, shard_id)| shard_id.clone())
                    .collect(),
                _ => Vec::new(),
            };
            self.shards.push(ShardRecord::from_parents(
                index,
                at,
                starting_hash_key,
                ending_hash_key,
                &covered,
            ));
        }
        open_before
    }

    /// Whether the stream's shard map holds a shard named `shard_id`, open or closed.
    pub fn holds_shard(&self, shard_id: &str) -> bool {
        self.shards.iter().any(|shard| shard.shard_id == shard_id)
    }

    /// The position in [`Self::shards`] of the OPEN shard named `shard_id`, if the map
    /// holds one: only an open shard still covers a slice of the hash key space, so only
    /// an open one can be split or merged.
    pub fn open_shard_index(&self, shard_id: &str) -> Option<usize> {
        self.shards
            .iter()
            .position(|shard| shard.shard_id == shard_id && shard.is_open())
    }

    /// Close the two open shards at `first` and `second` and cover their joined slice of
    /// the hash key space with ONE new open shard, answering that child's id.
    ///
    /// "After the merge, the single child shard receives data for all hash key values
    /// covered by the two parent shards" (`MergeShards`), and the child names both shards
    /// it was produced from (`Shard$ParentShardId` / `Shard$AdjacentParentShardId`). The
    /// shard count is being changed, so the stream owes the `ACTIVE` it goes back to — an
    /// instant the record stores rather than sleeps on.
    ///
    /// Bounded by the caller: both shards are resolved, checked to be open and checked to
    /// be adjacent BEFORE this closes either of them.
    pub fn merge_shards(&mut self, first: usize, second: usize, at: SystemTime) -> Option<String> {
        let bounds = |position: usize| -> Option<(u128, u128, String)> {
            let shard = self.shards.get(position)?;
            let (starting, ending) = shard.hash_key_bounds()?;
            Some((starting, ending, shard.shard_id.clone()))
        };
        let one = bounds(first)?;
        let two = bounds(second)?;
        // The child covers both ranges, so it starts where the lower parent starts and
        // ends where the upper one ends, whichever order the request named them in.
        let (lower, upper) = if one.0 <= two.0 { (one, two) } else { (two, one) };

        let index = u32::try_from(self.shards.len()).unwrap_or(u32::MAX);
        let child = ShardRecord::from_parents(
            index,
            at,
            lower.0.to_string(),
            upper.1.to_string(),
            &[lower.2, upper.2],
        );
        let child_shard_id = child.shard_id.clone();
        for position in [first, second] {
            if let Some(shard) = self.shards.get_mut(position) {
                shard.close_at(at);
            }
        }
        self.shards.push(child);
        self.updating_until = Some(at + RESHARD_DELAY);
        Some(child_shard_id)
    }

    /// Close the open shard at `index` and cover its slice of the hash key space with TWO
    /// new open shards, answering their ids lowest first.
    ///
    /// "The NewStartingHashKey hash key value and all higher hash key values in hash key
    /// range are distributed to one of the child shards. All the lower hash key values in
    /// the range are distributed to the other child shard"
    /// (`SplitShardInput$NewStartingHashKey`), and each child names the shard it was
    /// produced from. The shard count is being changed, so the stream owes the `ACTIVE` it
    /// goes back to — an instant the record stores rather than sleeps on.
    ///
    /// Bounded by the caller: the shard is resolved, checked to be open and
    /// `new_starting_hash_key` checked to fall strictly inside its range BEFORE this closes
    /// it, so neither child covers an empty slice.
    pub fn split_shard(
        &mut self,
        index: usize,
        new_starting_hash_key: u128,
        at: SystemTime,
    ) -> Option<(String, String)> {
        let (parent_shard_id, starting, ending) = {
            let shard = self.shards.get(index)?;
            let (starting, ending) = shard.hash_key_bounds()?;
            (shard.shard_id.clone(), starting, ending)
        };
        let first_index = u32::try_from(self.shards.len()).unwrap_or(u32::MAX);
        // Both children are a split's, so each names the ONE shard it was produced from.
        let parent = ::std::slice::from_ref(&parent_shard_id);
        let lower = ShardRecord::from_parents(
            first_index,
            at,
            starting.to_string(),
            new_starting_hash_key.checked_sub(1)?.to_string(),
            parent,
        );
        let upper = ShardRecord::from_parents(
            first_index.saturating_add(1),
            at,
            new_starting_hash_key.to_string(),
            ending.to_string(),
            parent,
        );
        let children = (lower.shard_id.clone(), upper.shard_id.clone());
        if let Some(shard) = self.shards.get_mut(index) {
            shard.close_at(at);
        }
        self.shards.push(lower);
        self.shards.push(upper);
        self.updating_until = Some(at + RESHARD_DELAY);
        Some(children)
    }

    /// The children of the shard `shard_id`, as `GetRecords` reports them — "the existing
    /// child shard of the current shard" (`ChildShard`), which is what a reader that has
    /// reached the end of a closed shard reads on from.
    pub fn child_shards_of(&self, shard_id: &str) -> Vec<ChildShard> {
        self.shards
            .iter()
            .filter(|shard| shard.is_child_of(shard_id))
            .map(|shard| ChildShard {
                shard_id: shard.shard_id.clone(),
                parent_shards: shard.parent_shard_ids(),
                hash_key_range: HashKeyRange {
                    starting_hash_key: shard.starting_hash_key.clone(),
                    ending_hash_key: shard.ending_hash_key.clone(),
                },
            })
            .collect()
    }

    /// Whether this account and region still hold the stream at `now`: a stream a delete
    /// armed is held, in `DELETING`, until the removal that delete owes falls due, and
    /// names no resource from then on. Derived from the stored instant, so a reader reaches
    /// the same answer as a writer without either of them having to prune the store.
    pub fn is_held(&self, now: SystemTime) -> bool {
        self.deleted_at.is_none_or(|due| now < due)
    }

    /// The stream's trim horizon at `now`: the oldest instant data is still retained for.
    ///
    /// "A stream's data records are accessible for 24 hours from the time they are added to
    /// the stream" by default, and for `RetentionPeriodHours` once that window is changed,
    /// so the horizon trails `now` by the stream's own retention window — never reaching
    /// back past the instant the stream was created, since nothing was retained before it.
    pub fn trim_horizon(&self, now: SystemTime) -> SystemTime {
        let hours = u64::try_from(self.retention_period_hours).unwrap_or_default();
        let window = Duration::from_secs(hours.saturating_mul(SECONDS_PER_HOUR));
        now.checked_sub(window)
            .filter(|horizon| *horizon > self.created_at)
            .unwrap_or(self.created_at)
    }

    /// How many of the stream's shards are open.
    pub fn open_shard_count(&self) -> i32 {
        i32::try_from(self.shards.iter().filter(|shard| shard.is_open()).count())
            .unwrap_or(i32::MAX)
    }

    /// The consumer the stream carries under `name`, if any: a consumer name is unique
    /// inside ONE stream ("For a given Kinesis data stream, each consumer must have a
    /// unique name", `RegisterStreamConsumerInput$ConsumerName`).
    pub fn consumer(&self, name: &str) -> Option<&ConsumerRecord> {
        self.consumers.iter().find(|consumer| consumer.name == name)
    }

    /// The consumer the stream carries under `arn`, if any.
    pub fn consumer_by_arn(&self, arn: &str) -> Option<&ConsumerRecord> {
        self.consumers.iter().find(|consumer| consumer.arn == arn)
    }

    /// [`Self::consumer_by_arn`] for a caller that WRITES the record it finds — the lookup a
    /// resource-policy write makes, so the consumer a write attaches a policy to is the one a
    /// read of that policy reports.
    pub fn consumer_by_arn_mut(&mut self, arn: &str) -> Option<&mut ConsumerRecord> {
        self.consumers
            .iter_mut()
            .find(|consumer| consumer.arn == arn)
    }

    /// Every consumer registered with the stream, ordered by name.
    ///
    /// A listing of them is paged by an opaque cursor that names the consumer the next page
    /// starts at ([`ConsumerPageToken`]), so the consumers a read reports need ONE total
    /// order that every page agrees on, and a consumer's name is unique inside its stream:
    /// the order is by name, whatever order the registrations arrived in.
    pub fn consumers_ordered(&self) -> Vec<&ConsumerRecord> {
        let mut consumers: Vec<&ConsumerRecord> = self.consumers.iter().collect();
        consumers.sort_by(|left, right| left.name.cmp(&right.name));
        consumers
    }

    /// How many of the stream's consumers are still in `CREATING` at `now`, which is what
    /// [`MAX_CONSUMERS_CREATING`] bounds.
    pub fn consumers_creating(&self, now: SystemTime) -> usize {
        self.consumers
            .iter()
            .filter(|consumer| consumer.status(now) == CONSUMER_CREATING)
            .count()
    }

    /// How many consumers are registered with the stream, which is what
    /// `StreamDescriptionSummary$ConsumerCount` reports.
    pub fn consumer_count(&self) -> i32 {
        i32::try_from(self.consumers.len()).unwrap_or(i32::MAX)
    }

    /// Register a consumer named `name` with the stream at `at` and answer the record that
    /// was stored. The caller has already refused a name the stream carries and a
    /// registration past the documented ceilings, so this only writes.
    pub fn register_consumer(
        &mut self,
        name: &str,
        at: SystemTime,
        tags: HashMap<String, String>,
    ) -> ConsumerRecord {
        let consumer = ConsumerRecord::registered_at(&self.arn, name, at, tags);
        self.consumers.push(consumer.clone());
        consumer
    }

    /// Take the consumer the stream carries under `name` off it, answering the record that
    /// was stored, or `None` when the stream carries no consumer under that name.
    ///
    /// The consumer stops being registered with the stream, so nothing of it is read any
    /// more: the name it held is free for another registration, and a registration under it
    /// mints a NEW consumer rather than reviving this one — "If you delete a consumer and
    /// then create a new one with the same name, it won't have the same ARN"
    /// ([`consumer_arn`]). The consumers left keep their registration order.
    pub fn remove_consumer(&mut self, name: &str) -> Option<ConsumerRecord> {
        let position = self
            .consumers
            .iter()
            .position(|consumer| consumer.name == name)?;
        Some(self.consumers.remove(position))
    }

    /// This record as the modelled `StreamSummary`, which is what a listing reports beside
    /// the stream's name.
    pub fn to_stream_summary(&self, now: SystemTime) -> StreamSummary {
        StreamSummary {
            stream_name: self.name.clone(),
            stream_arn: self.arn.clone(),
            stream_status: self.status(now).to_string(),
            stream_mode_details: Some(self.stream_mode_details()),
            stream_creation_timestamp: Some(timestamp(self.created_at)),
        }
    }

    /// The stream's tags as the modelled `Tag` list, ordered by key, through the one
    /// projection every tagged resource of this service is reported through
    /// ([`tags_ordered`]).
    pub fn tags_ordered(&self) -> Vec<Tag> {
        tags_ordered(&self.tags)
    }

    /// How many tags the stream would carry once `tags` were assigned to it, which is what
    /// [`MAX_STREAM_TAGS`] bounds, through the one reading every tagged resource of this
    /// service is held to ([`tag_count_with`]).
    pub fn tag_count_with(&self, tags: &HashMap<String, String>) -> usize {
        tag_count_with(&self.tags, tags)
    }

    /// Assign `tags` to the stream: a key the stream already carries takes the new value
    /// ("If tags have already been assigned to the stream, AddTagsToStream overwrites any
    /// existing tags that correspond to the specified tag keys"), and every tag the
    /// request does not name stays as it was.
    ///
    /// Bounded by the caller: a request that would take the stream past
    /// [`MAX_STREAM_TAGS`] is refused BEFORE this grows the stored map
    /// ([`Self::tag_count_with`]).
    pub fn assign_tags(&mut self, tags: impl IntoIterator<Item = (String, String)>) {
        self.tags.extend(tags);
    }

    /// Unassign the tags `keys` names from the stream: "Removed tags are deleted and cannot
    /// be recovered after this operation successfully completes... If you specify a tag that
    /// does not exist, it is ignored" (`RemoveTagsFromStream`), so a key the stream does not
    /// carry removes nothing and every tag the request does not name stays as it was.
    ///
    /// The counterpart of [`Self::assign_tags`] over the same stored map, so a removal can
    /// only ever take the stream further below the [`MAX_STREAM_TAGS`] ceiling.
    ///
    /// Deletes through the one rule every resource kind of this service deletes by
    /// ([`unassign_tags`]), the counterpart of the one projection they are all reported
    /// through.
    pub fn unassign_tags<'keys>(&mut self, keys: impl IntoIterator<Item = &'keys str>) {
        unassign_tags(&mut self.tags, keys);
    }

    /// The enhanced monitoring configuration the stream reports: one entry carrying the
    /// shard-level metrics that are enabled, which is an empty list until some are.
    fn enhanced_monitoring(&self) -> Vec<EnhancedMetrics> {
        vec![EnhancedMetrics {
            shard_level_metrics: Some(self.shard_level_metrics.clone()),
        }]
    }

    /// Turn the shard-level metrics `requested` ON for this stream, answering the set that
    /// was enhanced BEFORE the call.
    ///
    /// The stored set is a SET: the metrics the request names are added to the ones the
    /// stream already reports, so naming a metric twice, or one that is already enhanced,
    /// leaves the same set behind ([`requested_shard_level_metrics`] expands the catch-all
    /// and settles the order).
    pub fn enable_shard_level_metrics(&mut self, requested: &[String]) -> Vec<String> {
        let before = self.shard_level_metrics.clone();
        let added = requested_shard_level_metrics(requested);
        self.shard_level_metrics = shard_level_metric_catalog()
            .into_iter()
            .filter(|metric| {
                before.iter().any(|enhanced| enhanced == metric)
                    || added.iter().any(|name| name == metric)
            })
            .map(str::to_string)
            .collect();
        before
    }

    /// Turn the shard-level metrics `requested` OFF for this stream, answering the set that
    /// was enhanced BEFORE the call.
    ///
    /// The counterpart of [`Self::enable_shard_level_metrics`] over the same stored set:
    /// every metric the request names stops being enhanced and every other one stays as it
    /// was, so naming a metric nothing enhanced leaves the same set behind.
    pub fn disable_shard_level_metrics(&mut self, requested: &[String]) -> Vec<String> {
        let before = self.shard_level_metrics.clone();
        let removed = requested_shard_level_metrics(requested);
        self.shard_level_metrics = before
            .iter()
            .filter(|enhanced| !removed.iter().any(|name| name == *enhanced))
            .cloned()
            .collect();
        before
    }

    /// The warm throughput the stream reports, which only a stream of the capacity mode the
    /// configuration exists on has at all: "Represents the warm throughput configuration on
    /// the stream. This is only present for On-Demand Kinesis Data Streams"
    /// (`WarmThroughputObject`), so a stream whose shard map its owner sized reports NONE.
    ///
    /// For an on-demand stream it is the target a create or an update asked for, or the
    /// throughput its open shards provide ([`SHARD_WRITE_MI_BPS`] each). Both members carry
    /// the same value — the input member's doc calls it "the throughput capacity that will be
    /// immediately available for write operations".
    pub fn warm_throughput(&self) -> Option<WarmThroughputObject> {
        if self.stream_mode != STREAM_MODE_ON_DEMAND {
            return None;
        }
        let target = self
            .warm_throughput_mi_bps
            .unwrap_or_else(|| self.open_shard_count().saturating_mul(SHARD_WRITE_MI_BPS));
        Some(WarmThroughputObject {
            target_mi_bps: Some(target),
            current_mi_bps: Some(target),
        })
    }

    /// The `StreamModeDetails` the stream reports.
    fn stream_mode_details(&self) -> StreamModeDetails {
        StreamModeDetails {
            stream_mode: self.stream_mode.clone(),
        }
    }

    /// This record as the modelled `StreamDescription`, carrying `shards` and whether the
    /// shard map holds more beyond them.
    pub fn to_description(
        &self,
        now: SystemTime,
        shards: Vec<Shard>,
        has_more_shards: bool,
    ) -> StreamDescription {
        StreamDescription {
            stream_name: self.name.clone(),
            stream_arn: self.arn.clone(),
            stream_status: self.status(now).to_string(),
            stream_mode_details: Some(self.stream_mode_details()),
            shards,
            has_more_shards,
            retention_period_hours: self.retention_period_hours,
            stream_creation_timestamp: timestamp(self.created_at),
            enhanced_monitoring: self.enhanced_monitoring(),
            encryption_type: Some(self.encryption_type.clone()),
            key_id: self.key_id.clone(),
        }
    }

    /// This record as the modelled `StreamDescriptionSummary`.
    pub fn to_summary(&self, now: SystemTime) -> StreamDescriptionSummary {
        StreamDescriptionSummary {
            stream_name: self.name.clone(),
            stream_arn: self.arn.clone(),
            // `StreamDescriptionSummary$StreamId` is documented "Not Implemented.
            // Reserved for future use", so no stream carries one.
            stream_id: None,
            stream_status: self.status(now).to_string(),
            stream_mode_details: Some(self.stream_mode_details()),
            retention_period_hours: self.retention_period_hours,
            stream_creation_timestamp: timestamp(self.created_at),
            enhanced_monitoring: self.enhanced_monitoring(),
            encryption_type: Some(self.encryption_type.clone()),
            key_id: self.key_id.clone(),
            open_shard_count: self.open_shard_count(),
            // Every consumer `RegisterStreamConsumer` registered with the stream is counted
            // here; no channel is associated until `CreateChannel` creates one.
            consumer_count: Some(self.consumer_count()),
            warm_throughput: self.warm_throughput(),
            // Every stream is configured for a maximum record size, so every summary
            // reports one: the value a create or an update named, or the default the
            // stream fell back to.
            max_record_size_in_ki_b: Some(self.max_record_size_in_ki_b),
            channel_count: Some(0),
            // "This field is only present for streams that use the on-demand capacity mode"
            // (`StreamDescriptionSummary$RecordDistributionStrategy`).
            record_distribution_strategy: self.reported_record_distribution(),
        }
    }
}

/// One stored Kinesis channel: the authoritative record every channel operation reads.
/// `CreateChannel` inserts it and `DescribeChannel` projects it onto its output shape.
///
/// The destinations and the logging configuration are stored as the DESCRIPTION shapes,
/// with every documented default already resolved, so what a create accepted is exactly
/// what every later read reports.
#[derive(Debug, Clone)]
pub struct Channel {
    /// The `ChannelName` the create carried, which is half of this record's key.
    pub name: String,
    /// The region the creating request was signed for.
    pub region: String,
    /// The `ChannelARN`, as [`channel_arn`] builds it.
    pub arn: String,
    /// The `ChannelId`, as [`channel_id`] derives it.
    pub id: String,
    /// When `CreateChannel` minted the channel.
    pub created_at: SystemTime,
    /// When the channel's `CREATING` -> `ACTIVE` transition falls due: "Upon receiving the
    /// request, Amazon Kinesis Data Streams returns immediately with the channel in the
    /// CREATING state. After provisioning is complete, Amazon Kinesis Data Streams sets the
    /// state to ACTIVE" (`CreateChannel`).
    pub active_at: SystemTime,
    /// The `ServiceExecutionRoleARN` the create carried.
    pub service_execution_role_arn: String,
    /// The channel's source streams, each carrying the stored stream's own creation instant.
    pub streams: Vec<ChannelStreamDescription>,
    /// The general purpose bucket destination, when the create named one.
    pub s3_destination: Option<S3DestinationDescription>,
    /// The streaming table destination, when the create named one.
    pub s3_tables_destination: Option<S3TablesDestinationDescription>,
    /// The server-side encryption configuration, when the create named one.
    pub encryption_configuration: Option<ChannelEncryptionConfiguration>,
    /// The CloudWatch Logs configuration, with the documented default names resolved.
    pub logging_configuration: ChannelLoggingConfiguration,
    /// The tags the create carried.
    pub tags: HashMap<String, String>,
    /// When the channel's `UPDATING` -> `ACTIVE` transition falls due, once a configuration
    /// change has been accepted for it: "Upon receiving the request, Amazon Kinesis Data
    /// Streams sets the channel to the UPDATING state and returns immediately. After the
    /// change is applied, Amazon Kinesis Data Streams sets the channel back to the ACTIVE
    /// state" (`UpdateChannel`). `None` on a channel no change has been accepted for.
    pub updating_until: Option<SystemTime>,
    /// The resource-based policy attached to the channel, `None` while none is. A
    /// `ResourceARN` the resource-policy operations address a resource by names a resource of
    /// either kind this service holds
    /// (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`), so a channel holds one the
    /// same way a stream does.
    pub policy: Option<String>,
}

impl Channel {
    /// The channel's status at `now`: `CREATING` until the transition the create armed falls
    /// due, `UPDATING` until the one an accepted configuration change armed falls due,
    /// `ACTIVE` from then on. Derived from the stored instants, so two reads at the same
    /// instant agree and no read has to write.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.active_at {
            CHANNEL_CREATING
        } else if self.updating_until.is_some_and(|due| now < due) {
            CHANNEL_UPDATING
        } else {
            CHANNEL_ACTIVE
        }
    }

    /// Where the channel delivers, as `ChannelDestinationType` spells it: "S3 - Delivery to
    /// a general purpose Amazon S3 bucket", "S3_TABLES - Delivery to streaming tables on
    /// Apache Iceberg" (`ChannelSummary$ChannelDestinationType`). Derived from the stored
    /// destination, exactly one of which a channel carries — which is also the one thing an
    /// update cannot change: "You cannot change the destination ... of an existing channel"
    /// (`UpdateChannel`).
    pub fn destination_type(&self) -> &'static str {
        if self.s3_tables_destination.is_some() {
            CHANNEL_DESTINATION_S3_TABLES
        } else {
            CHANNEL_DESTINATION_S3
        }
    }

    /// The stored record projected onto `ChannelSummary`, which is what `ListChannels`
    /// reports: "A summary of a channel". The members it shares with
    /// [`Self::to_description`] come from the same stored fields, so a listing and a read of
    /// one channel never disagree, and its source streams are reported as the identifier
    /// pair (`ChannelStreamIdentifier`) a summary carries rather than the fuller
    /// description.
    pub fn to_summary(&self, now: SystemTime) -> ChannelSummary {
        ChannelSummary {
            channel_name: self.name.clone(),
            channel_arn: self.arn.clone(),
            channel_id: self.id.clone(),
            channel_status: self.status(now).to_string(),
            // See [`Self::to_description`]: a channel this emulator holds provisions, so
            // none of them is `FAILED` and none carries a reason.
            channel_status_reason: None,
            channel_creation_timestamp: timestamp(self.created_at),
            channel_destination_type: self.destination_type().to_string(),
            streams: self
                .streams
                .iter()
                .map(|stream| ChannelStreamIdentifier {
                    stream_arn: stream.stream_arn.clone(),
                    stream_creation_timestamp: stream.stream_creation_timestamp,
                })
                .collect(),
        }
    }

    /// The channel's tags as the modelled `Tag` list, ordered by key — the SAME projection
    /// of the SAME kind of stored tag set a stream's tags are reported through
    /// ([`Stream::tags_ordered`]), because one rule orders the tags of any Kinesis resource.
    pub fn tags_ordered(&self) -> Vec<Tag> {
        tags_ordered(&self.tags)
    }

    /// Whether one of the channel's source streams is the stream `stream_arn` names, created
    /// at `created_at` when the caller pinned an instant.
    ///
    /// An ARN a delete released and a later create took again names another stream, so a
    /// caller that pins the creation instant is answered about THAT incarnation only; one
    /// that pins none is answered about whichever stream the ARN holds. Instants are compared
    /// at the millisecond precision this service's stored instants carry.
    pub fn sources_stream(
        &self,
        stream_arn: &str,
        created_at: Option<::aws_smithy_types::DateTime>,
    ) -> bool {
        self.streams.iter().any(|stream| {
            stream.stream_arn == stream_arn
                && created_at.is_none_or(|pinned| {
                    pinned.to_millis().ok() == stream.stream_creation_timestamp.to_millis().ok()
                })
        })
    }

    /// The stored record projected onto `ChannelDescription`, which is what `CreateChannel`
    /// and `DescribeChannel` both answer with: "The configuration and current status of the
    /// channel, including its ARN, destination configuration, and lifecycle state".
    pub fn to_description(&self, now: SystemTime) -> ChannelDescription {
        ChannelDescription {
            channel_name: self.name.clone(),
            channel_arn: self.arn.clone(),
            channel_id: self.id.clone(),
            channel_status: self.status(now).to_string(),
            // "A message describing the reason for a FAILED status"
            // (`ChannelDescription$ChannelStatusReason`) — a channel this emulator holds
            // provisions, so none of them is `FAILED` and none carries a reason.
            channel_status_reason: None,
            channel_creation_timestamp: timestamp(self.created_at),
            service_execution_role_arn: self.service_execution_role_arn.clone(),
            stream_configuration_list: self.streams.clone(),
            s3_destination_configuration: self.s3_destination.clone(),
            s3_tables_destination_configuration: self.s3_tables_destination.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
            logging_configuration: self.logging_configuration.clone(),
        }
    }
}

/// The minimum throughput billing commitment this account holds: the instant it started, the
/// earliest instant it may end at, and the instant it ends at once an end has been reached or
/// scheduled.
///
/// `UpdateAccountSettings` writes it and `DescribeAccountSettings` reports it, both through
/// [`Self::to_shape`]. [`Self::status`] is the single place where the `ENABLED` ->
/// `ENABLED_UNTIL_EARLIEST_ALLOWED_END` -> `DISABLED` transitions are decided, so the end the
/// clock owes a scheduled disable is materialised by whoever reads it rather than slept on.
#[derive(Debug, Clone)]
pub struct Commitment {
    /// "The timestamp when the commitment was started"
    /// (`MinimumThroughputBillingCommitmentOutput$StartedAt`).
    pub started_at: SystemTime,
    /// "The earliest timestamp when the commitment can be ended" (same shape,
    /// `EarliestAllowedEndAt`): one [`MINIMUM_COMMITMENT_DURATION`] past the start.
    pub earliest_allowed_end_at: SystemTime,
    /// The instant this commitment ends at, once an end has been reached or scheduled;
    /// `None` on a commitment no disable has been accepted for.
    pub ends_at: Option<SystemTime>,
}

impl Commitment {
    /// The commitment an enable starts at `at`, which stands until it is both disabled and
    /// past the minimum duration it has to honour.
    pub fn started_at(at: SystemTime) -> Self {
        Self {
            started_at: at,
            earliest_allowed_end_at: at + MINIMUM_COMMITMENT_DURATION,
            ends_at: None,
        }
    }

    /// The status this commitment carries at `now`: one no disable has been accepted for is
    /// `ENABLED`, one whose end is still ahead stands until it
    /// (`ENABLED_UNTIL_EARLIEST_ALLOWED_END`), and one whose end has fallen due is
    /// `DISABLED`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        match self.ends_at {
            None => COMMITMENT_ENABLED,
            Some(ends_at) if ends_at <= now => COMMITMENT_DISABLED,
            Some(_) => COMMITMENT_ENABLED_UNTIL_EARLIEST_ALLOWED_END,
        }
    }

    /// The instant this commitment HAS ended at, which is what "The timestamp when the
    /// commitment was ended" (`MinimumThroughputBillingCommitmentOutput$EndedAt`) names: a
    /// commitment whose scheduled end is still ahead has not ended, so it carries none.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.ends_at.filter(|ends_at| *ends_at <= now)
    }

    /// Cancel the end a disable scheduled for this commitment: "You can cancel a pending
    /// disable by enabling the commitment again before the earliest allowed end time"
    /// (`MinimumThroughputBillingCommitmentInput`). A commitment no disable has been accepted
    /// for is left exactly as it stands, so enabling twice starts no second window.
    pub fn cancel_end(&mut self) {
        self.ends_at = None;
    }

    /// Schedule this commitment's end at the earliest instant it may end at, which is `now`
    /// once the minimum duration it has to honour has passed and the earliest allowed end
    /// otherwise. A commitment whose end is already scheduled or reached keeps that end, so
    /// disabling twice brings it no closer.
    pub fn schedule_end(&mut self, now: SystemTime) {
        if self.ends_at.is_none() {
            self.ends_at = Some(self.earliest_allowed_end_at.max(now));
        }
    }

    /// This record as the modelled `MinimumThroughputBillingCommitmentOutput`, which is what
    /// both account-settings operations answer with.
    pub fn to_shape(&self, now: SystemTime) -> MinimumThroughputBillingCommitmentOutput {
        MinimumThroughputBillingCommitmentOutput {
            status: self.status(now).to_string(),
            started_at: Some(timestamp(self.started_at)),
            ended_at: self.ended_at(now).map(timestamp),
            earliest_allowed_end_at: Some(timestamp(self.earliest_allowed_end_at)),
        }
    }
}

/// What the account-settings operations report for an account that has never asked for a
/// minimum throughput billing commitment: a commitment is something an account opts into, so
/// none stands and there is no instant to report for one.
pub fn no_commitment() -> MinimumThroughputBillingCommitmentOutput {
    MinimumThroughputBillingCommitmentOutput {
        status: COMMITMENT_DISABLED.to_string(),
        started_at: None,
        ended_at: None,
        earliest_allowed_end_at: None,
    }
}

/// Per-service state for com-amazonaws-kinesis-kinesis-20131202. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisKinesis20131202State {
    /// Every stream this emulator holds, keyed by region and name.
    pub streams: HashMap<RegionalName, Stream>,
    /// Every channel this emulator holds, keyed by region and name: "The name is unique
    /// within your Amazon Web Services account and Amazon Web Services Region"
    /// (`CreateChannelInput$ChannelName`).
    pub channels: HashMap<RegionalName, Channel>,
    /// The minimum throughput billing commitment this account holds, `None` while it has
    /// never asked for one. ONE record, not one per region: what `UpdateAccountSettings`
    /// writes is "your account's updated settings" and what `DescribeAccountSettings` reports
    /// is "the minimum throughput billing commitment for your Amazon Web Services account",
    /// so every endpoint of the one account this emulator holds reads the same commitment.
    pub commitment: Option<Commitment>,
}

impl ComAmazonawsKinesisKinesis20131202State {
    /// The stream `region`'s endpoint holds under `name` at `now`, if any.
    pub fn stream_in(&self, now: SystemTime, region: &str, name: &str) -> Option<&Stream> {
        self.streams
            .get(&RegionalName::new(region, name))
            .filter(|stream| stream.is_held(now))
    }

    /// The stream `region`'s endpoint holds under `arn` at `now`, if any.
    pub fn stream_by_arn(&self, now: SystemTime, region: &str, arn: &str) -> Option<&Stream> {
        self.streams
            .values()
            .find(|stream| stream.region == region && stream.arn == arn)
            .filter(|stream| stream.is_held(now))
    }

    /// Every stream `region`'s endpoint holds at `now`, ordered by name.
    ///
    /// A listing is paged by name ("you can request more streams by using the name of the
    /// last stream returned by the ListStreams request in the ExclusiveStartStreamName
    /// parameter", `ListStreams`), so the streams a read reports need ONE total order that
    /// every page agrees on, and the cursor names the stream: the order is by name.
    pub fn streams_ordered_in(&self, now: SystemTime, region: &str) -> Vec<&Stream> {
        let mut streams: Vec<&Stream> = self
            .streams
            .values()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .collect();
        streams.sort_by(|left, right| left.name.cmp(&right.name));
        streams
    }

    /// The stream a request's `StreamName` / `StreamARN` pair names at `now`.
    ///
    /// "When invoking this API, you must use either the StreamARN or the StreamName
    /// parameter, or both" (`DescribeStream`), and that holds for every operation of this
    /// service that addresses a stream: a request naming neither is refused rather than
    /// answered, and a request naming both has to name ONE stream with them.
    pub fn resolve_stream(
        &self,
        now: SystemTime,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<&Stream, StreamLookupFailure> {
        match (name, arn) {
            (None, None) => Err(StreamLookupFailure::Unspecified),
            (Some(name), None) => self
                .stream_in(now, region, name)
                .ok_or(StreamLookupFailure::NotFound),
            (None, Some(arn)) => self
                .stream_by_arn(now, region, arn)
                .ok_or(StreamLookupFailure::NotFound),
            (Some(name), Some(arn)) => {
                // The pair is reconciled FROM THE REQUEST, before any stream is looked up:
                // what has to agree is the `StreamName` and the name the `StreamARN` carries
                // in its `stream/<name>` segment, so a disagreeing pair is refused as an
                // invalid argument whichever of the two names a held stream — and an agreeing
                // pair is resolved by the ARN exactly as one naming the ARN alone is.
                if stream_name_in(arn) != name {
                    return Err(StreamLookupFailure::Mismatch {
                        name: name.to_string(),
                        arn: arn.to_string(),
                    });
                }
                self.stream_by_arn(now, region, arn)
                    .ok_or(StreamLookupFailure::NotFound)
            }
        }
    }

    /// The key of the stream a request's `StreamName` / `StreamARN` pair names, resolved
    /// by exactly the rule [`Self::resolve_stream`] applies.
    fn resolve_stream_key(
        &self,
        now: SystemTime,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<RegionalName, StreamLookupFailure> {
        self.resolve_stream(now, region, name, arn)
            .map(|stream| RegionalName::new(&stream.region, &stream.name))
    }

    /// The stream a request's `StreamName` / `StreamARN` pair names, for an operation that
    /// WRITES to it. One rule, one lookup: the pair is resolved by
    /// [`Self::resolve_stream_key`] and the stored record is then reached by its key.
    pub fn resolve_stream_mut(
        &mut self,
        now: SystemTime,
        region: &str,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<&mut Stream, StreamLookupFailure> {
        let key = self.resolve_stream_key(now, region, name, arn)?;
        self.streams
            .get_mut(&key)
            .ok_or(StreamLookupFailure::NotFound)
    }

    /// The stream and the consumer a `DescribeStreamConsumer`-shaped request names at
    /// `now`.
    ///
    /// "To get the description of a registered consumer, provide the ARN of the consumer.
    /// Alternatively, you can provide the ARN of the data stream and the name you gave the
    /// consumer when you registered it. You may also provide all three parameters, as long
    /// as they don't conflict with each other" (`DescribeStreamConsumer`): a request naming
    /// neither the consumer's ARN nor the stream-and-name pair is refused rather than
    /// answered, and a request naming both has to name ONE consumer with them.
    pub fn resolve_consumer(
        &self,
        now: SystemTime,
        region: &str,
        stream_arn: Option<&str>,
        consumer_name: Option<&str>,
        consumer_arn: Option<&str>,
    ) -> Result<(&Stream, &ConsumerRecord), ConsumerLookupFailure> {
        match (consumer_arn, stream_arn) {
            (Some(consumer_arn), _) => {
                let (stream, consumer) = self
                    .streams
                    .values()
                    .filter(|stream| stream.region == region && stream.is_held(now))
                    .find_map(|stream| {
                        stream
                            .consumer_by_arn(consumer_arn)
                            .map(|consumer| (stream, consumer))
                    })
                    .ok_or(ConsumerLookupFailure::ConsumerNotFound)?;
                // The members a request names beside the consumer's ARN must not conflict
                // with the consumer that ARN resolves to.
                if stream_arn.is_some_and(|arn| arn != stream.arn)
                    || consumer_name.is_some_and(|name| name != consumer.name)
                {
                    return Err(ConsumerLookupFailure::Mismatch);
                }
                Ok((stream, consumer))
            }
            (None, Some(stream_arn)) => {
                let consumer_name =
                    consumer_name.ok_or(ConsumerLookupFailure::ConsumerUnspecified)?;
                let stream = self
                    .stream_by_arn(now, region, stream_arn)
                    .ok_or(ConsumerLookupFailure::StreamNotFound)?;
                let consumer = stream
                    .consumer(consumer_name)
                    .ok_or(ConsumerLookupFailure::ConsumerNotFound)?;
                Ok((stream, consumer))
            }
            (None, None) => Err(ConsumerLookupFailure::ConsumerUnspecified),
        }
    }

    /// The key of the stream carrying the consumer a request's `ConsumerARN` / `StreamARN` /
    /// `ConsumerName` members name, and that consumer's name, resolved by exactly the rule
    /// [`Self::resolve_consumer`] applies.
    fn resolve_consumer_key(
        &self,
        now: SystemTime,
        region: &str,
        stream_arn: Option<&str>,
        consumer_name: Option<&str>,
        consumer_arn: Option<&str>,
    ) -> Result<(RegionalName, String), ConsumerLookupFailure> {
        self.resolve_consumer(now, region, stream_arn, consumer_name, consumer_arn)
            .map(|(stream, consumer)| {
                (
                    RegionalName::new(&stream.region, &stream.name),
                    consumer.name.clone(),
                )
            })
    }

    /// Take the consumer a `DeregisterStreamConsumer`-shaped request names off the stream it
    /// is registered with, answering the record that was stored.
    ///
    /// One rule, one lookup: the request's members are resolved by
    /// [`Self::resolve_consumer_key`], so a deregistration addresses the same consumer a read
    /// does, and the stored record is then reached by its key and removed
    /// ([`Stream::remove_consumer`]) — the consumer is no longer registered with the stream,
    /// which is what every read of it and the stream's consumer count then report.
    pub fn deregister_consumer(
        &mut self,
        now: SystemTime,
        region: &str,
        stream_arn: Option<&str>,
        consumer_name: Option<&str>,
        consumer_arn: Option<&str>,
    ) -> Result<ConsumerRecord, ConsumerLookupFailure> {
        let (key, name) =
            self.resolve_consumer_key(now, region, stream_arn, consumer_name, consumer_arn)?;
        self.streams
            .get_mut(&key)
            .ok_or(ConsumerLookupFailure::StreamNotFound)?
            .remove_consumer(&name)
            .ok_or(ConsumerLookupFailure::ConsumerNotFound)
    }

    /// Drop every stream whose removal has fallen due at `now`, freeing its shard logs.
    ///
    /// The removal a `DeleteStream` armed is what makes the stream stop naming a resource
    /// ([`Stream::is_held`]), which every lookup decides from the stored instant — so this
    /// changes what no caller sees and only hands the memory back. Called by the operations
    /// that change which streams this account holds, which are the ones whose ceilings are
    /// counted against the store.
    pub fn drop_removed_streams(&mut self, now: SystemTime) {
        self.streams.retain(|_, stream| stream.is_held(now));
    }

    /// How many shards `region`'s endpoint holds at `now` across every stream, open and
    /// closed alike — the shards the store is actually carrying, which is what the memory
    /// ceiling [`MAX_SHARDS_TOTAL`] is counted against.
    pub fn shards_held_in(&self, now: SystemTime, region: &str) -> u32 {
        self.streams
            .values()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .fold(0u32, |held, stream| {
                held.saturating_add(u32::try_from(stream.shards.len()).unwrap_or(u32::MAX))
            })
    }

    /// How many of the shards `region`'s endpoint holds at `now` are OPEN, across every
    /// stream it holds.
    ///
    /// A shard a re-tiling closed still carries the records written to it, so the store
    /// holds it ([`Self::shards_held_in`]) while it provides no capacity any more — and it
    /// is the OPEN ones that are the region's shard usage, which is what `DescribeLimits`
    /// reports as `OpenShardCount` ("The number of open shards").
    pub fn open_shards_held_in(&self, now: SystemTime, region: &str) -> i32 {
        self.streams
            .values()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .fold(0i32, |open, stream| {
                open.saturating_add(stream.open_shard_count())
            })
    }

    /// How many streams of the on-demand capacity mode `region`'s endpoint holds at `now`,
    /// which is what [`MAX_ON_DEMAND_STREAMS_PER_REGION`] bounds and what `DescribeLimits`
    /// reports as `OnDemandStreamCount`.
    pub fn on_demand_streams_held_in(&self, now: SystemTime, region: &str) -> usize {
        self.streams
            .values()
            .filter(|stream| {
                stream.region == region
                    && stream.is_held(now)
                    && stream.stream_mode == STREAM_MODE_ON_DEMAND
            })
            .count()
    }

    /// How many shards this emulator holds at `now` across every region, which is what
    /// [`MAX_SHARDS_TOTAL`] bounds: the ceiling no request can partition.
    pub fn shards_held(&self, now: SystemTime) -> u32 {
        self.streams
            .values()
            .filter(|stream| stream.is_held(now))
            .fold(0u32, |held, stream| {
                held.saturating_add(u32::try_from(stream.shards.len()).unwrap_or(u32::MAX))
            })
    }

    /// This account and region's shard quota at `now`, and the usage held against it:
    /// `(ShardLimit, OpenShardCount)`, exactly as `DescribeLimits` answers with them.
    ///
    /// ONE number, read here by the operation that reports it and by every operation that
    /// opens a shard, so the quota a caller is told about is the quota it is refused past:
    /// a request that would open at most `ShardLimit - OpenShardCount` more shards is
    /// served, and one for more than that is refused
    /// ([`shard_limit_exceeded_message`]).
    ///
    /// The quota is per account and region, so the usage is the region's open shard count
    /// ([`Self::open_shards_held_in`]). The emulator's own memory ceiling across every
    /// region ([`MAX_SHARDS_TOTAL`]) is folded in rather than kept as a second, hidden
    /// ceiling: it narrows the quota REPORTED once the store runs short, so the two
    /// surfaces never contradict each other.
    pub fn shard_quota_in(&self, now: SystemTime, region: &str) -> (i32, i32) {
        let open = self.open_shards_held_in(now, region);
        let region_ceiling = i32::try_from(MAX_SHARDS_PER_REGION).unwrap_or(i32::MAX);
        let store_ceiling = i32::try_from(MAX_SHARDS_TOTAL).unwrap_or(i32::MAX);
        let held = i32::try_from(self.shards_held(now)).unwrap_or(i32::MAX);
        // What the account may still open here: the room under its own quota, and never
        // more room than the store has left anywhere.
        let room = region_ceiling
            .saturating_sub(open)
            .min(store_ceiling.saturating_sub(held))
            .max(0);
        (open.saturating_add(room), open)
    }

    /// The channel `region`'s endpoint holds under `name`, if any — the lookup a create
    /// makes to tell a second record from a conflict.
    pub fn channel_in(&self, region: &str, name: &str) -> Option<&Channel> {
        self.channels.get(&RegionalName::new(region, name))
    }

    /// The channel `region`'s endpoint holds under `arn`, if any: `ChannelARN` is the only
    /// member every channel read and write addresses a channel by.
    pub fn channel_by_arn(&self, region: &str, arn: &str) -> Option<&Channel> {
        self.channels
            .values()
            .find(|channel| channel.region == region && channel.arn == arn)
    }

    /// [`Self::channel_by_arn`] for a caller that WRITES the record it finds — the lookup a
    /// configuration change makes, so the status it reads and the fields it writes belong to
    /// one borrow of the store and no second change can slip between them.
    pub fn channel_by_arn_mut(&mut self, region: &str, arn: &str) -> Option<&mut Channel> {
        self.channels
            .values_mut()
            .find(|channel| channel.region == region && channel.arn == arn)
    }

    /// How many channels `region`'s endpoint holds, which is what
    /// [`MAX_CHANNELS_PER_REGION`] bounds.
    pub fn channels_held_in(&self, region: &str) -> usize {
        self.channels
            .keys()
            .filter(|key| key.region == region)
            .count()
    }

    /// How many channels this emulator holds across every region, which is what
    /// [`MAX_CHANNELS_TOTAL`] bounds: the ceiling no request can partition.
    pub fn channels_held(&self) -> usize {
        self.channels.len()
    }

    /// The minimum throughput billing commitment this account holds at `now`, as the shape
    /// both account-settings operations report it with. An account that has never asked for
    /// one reports [`no_commitment`].
    pub fn commitment_at(&self, now: SystemTime) -> MinimumThroughputBillingCommitmentOutput {
        self.commitment
            .as_ref()
            .map(|commitment| commitment.to_shape(now))
            .unwrap_or_else(no_commitment)
    }

    /// Take this account's minimum throughput billing commitment to the status the request
    /// asks for at `now`, and answer what it carries once the change is stored.
    ///
    /// Enabling holds a commitment: it starts one when none stands, and cancels the end a
    /// disable scheduled for one that does. Disabling ends one: at `now` when the minimum
    /// duration it has to honour has passed, and at its earliest allowed end otherwise.
    /// Either way, asking for the status the account already carries changes nothing.
    pub fn set_commitment(
        &mut self,
        now: SystemTime,
        enabled: bool,
    ) -> MinimumThroughputBillingCommitmentOutput {
        match (&mut self.commitment, enabled) {
            // A commitment that has already ended is spent: enabling starts a NEW one, whose
            // own minimum duration runs from this instant. One that still stands keeps the
            // start and the earliest allowed end it was created with, and only loses the end a
            // disable had scheduled for it.
            (Some(commitment), true) if commitment.status(now) != COMMITMENT_DISABLED => {
                commitment.cancel_end();
            }
            (_, true) => self.commitment = Some(Commitment::started_at(now)),
            (Some(commitment), false) => commitment.schedule_end(now),
            // Nothing to end, and nothing to record either: an account that has never asked
            // for a commitment holds none whether it asks for that or not.
            (None, false) => {}
        }
        self.commitment_at(now)
    }

    /// Every channel `region`'s endpoint holds, ordered by name.
    ///
    /// A listing of them is paginated ("The results are paginated. Use the NextToken value
    /// returned in the response to retrieve additional results", `ListChannels`), so the
    /// channels a read reports need ONE total order every page agrees on; a channel is
    /// addressed by a name unique in its region, so that order is by name.
    pub fn channels_ordered_in(&self, region: &str) -> Vec<&Channel> {
        let mut channels: Vec<&Channel> = self
            .channels
            .values()
            .filter(|channel| channel.region == region)
            .collect();
        channels.sort_by(|left, right| left.name.cmp(&right.name));
        channels
    }

    /// The channels this store holds that source the stream `stream_arn` names, in the order
    /// their names sort in.
    ///
    /// "The list of Kinesis data streams that are sources for this channel"
    /// (`ChannelDescription$StreamConfigurationList`): a stream named there is a DEPENDENCY
    /// of that channel, which is what stands in the way of the stream's removal. The ARN is
    /// the identifier both sides hold, so one walk answers it whatever region the channel was
    /// created in.
    pub fn channels_sourcing(&self, stream_arn: &str) -> Vec<&Channel> {
        let mut channels: Vec<&Channel> = self
            .channels
            .values()
            .filter(|channel| channel.sources_stream(stream_arn, None))
            .collect();
        channels.sort_by(|left, right| left.name.cmp(&right.name));
        channels
    }

    /// The tags the Kinesis resource `arn` names carries at `now`, ordered by key, or `None`
    /// when this account and region hold no resource under that ARN.
    ///
    /// A `ResourceARN` addresses "the Kinesis resource" rather than one kind of it
    /// (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`), and a stream and a channel
    /// each carry ONE stored tag set, so ONE lookup spans both kinds: whichever kind the ARN
    /// names, the tags answered are the stored set every tag write of that resource wrote.
    pub fn resource_tags_by_arn(
        &self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<Vec<Tag>> {
        self.stream_by_arn(now, region, arn)
            .map(Stream::tags_ordered)
            .or_else(|| self.channel_by_arn(region, arn).map(Channel::tags_ordered))
    }

    /// The resource-policy slot of the Kinesis resource `arn` names at `now` — `Some(None)`
    /// for a resource carrying no policy — or `None` when this account and region hold no
    /// resource under that ARN.
    ///
    /// A policy is "attached to the specified data stream or consumer"
    /// (`GetResourcePolicy`) and the `ResourceARN` it is addressed by names a resource of
    /// either kind this ARN may name
    /// (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`), so ONE lookup spans every
    /// kind of resource this service holds, exactly as [`Self::resource_tags_by_arn`] does:
    /// whichever kind the ARN names, the policy answered is the one stored on that very
    /// resource.
    /// The stream this account and region hold at `now` that the resource `arn` names
    /// belongs to: the stream its own `StreamARN` names, or the stream a `ConsumerARN`
    /// extends (`…stream/<name>/consumer/<name>:<timestamp>`).
    ///
    /// The ONE walk every resource-policy lookup of a stream-borne resource resolves through,
    /// so a read, a write and the state gate in front of them all address the same record.
    fn stream_holding_resource(
        &self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<&Stream> {
        self.streams
            .values()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .find(|stream| stream.arn == arn || stream.consumer_by_arn(arn).is_some())
    }

    pub fn resource_policy_by_arn(
        &self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<&Option<String>> {
        if let Some(channel) = self.channel_by_arn(region, arn) {
            return Some(&channel.policy);
        }
        let stream = self.stream_holding_resource(now, region, arn)?;
        if stream.arn == arn {
            Some(&stream.policy)
        } else {
            stream.consumer_by_arn(arn).map(|consumer| &consumer.policy)
        }
    }

    /// Whether the Kinesis resource `arn` names at `now` is in a state that SERVES a
    /// resource-policy operation, or the refusal that stands in the way: `None` when this
    /// account and region hold no resource under that ARN, `Some(Err(sentence))` when the
    /// resource is held in a state that does not serve the call, and `Some(Ok(()))` when it
    /// does.
    ///
    /// The precondition the three resource-policy operations share: "The resource is not
    /// available for this operation. For successful operation, the resource must be in the
    /// ACTIVE state" (`ResourceInUseException`), read for a data stream the same way every
    /// other operation of this service reads it — a stream whose change is still in flight
    /// keeps serving ("You can continue to read and write data to your stream while its
    /// status is UPDATING", `UpdateShardCount`), and a stream whose create or whose removal
    /// is still due serves nothing. A consumer's policy is addressed through the stream that
    /// holds it, so the SAME gate stands in front of it.
    ///
    /// Resolved by exactly the rule [`Self::resource_policy_by_arn`] resolves an ARN by, so
    /// the resource a gate reads the state of is the resource whose policy slot the call
    /// would then reach.
    pub fn resource_policy_readiness(
        &self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<Result<(), String>> {
        // A channel carries its own lifecycle states, which this precondition is not
        // published for: its policy slot is reached in every state it is held in.
        if self.channel_by_arn(region, arn).is_some() {
            return Some(Ok(()));
        }
        self.stream_holding_resource(now, region, arn).map(|stream| {
            let status = stream.status(now);
            if matches!(status, STREAM_ACTIVE | STREAM_UPDATING) {
                Ok(())
            } else {
                Err(stream_not_active_or_updating_message(&stream.name, status))
            }
        })
    }

    /// [`Self::resource_policy_by_arn`] for a caller that WRITES the slot it finds: one rule,
    /// one lookup, so a policy an attach writes and a detach clears is the very one a read
    /// reports.
    pub fn resource_policy_by_arn_mut(
        &mut self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<&mut Option<String>> {
        // Which kind the ARN names is decided through the immutable lookups, so a write
        // addresses a resource by exactly the rule a read does; only the record it resolved
        // is then borrowed mutably.
        if self.channel_by_arn(region, arn).is_some() {
            return self
                .channel_by_arn_mut(region, arn)
                .map(|channel| &mut channel.policy);
        }
        self.streams
            .values_mut()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .find_map(|stream| {
                if stream.arn == arn {
                    Some(&mut stream.policy)
                } else {
                    stream
                        .consumer_by_arn_mut(arn)
                        .map(|consumer| &mut consumer.policy)
                }
            })
    }

    /// [`Self::resource_tags_by_arn`] for a caller that WRITES the set it finds: one rule,
    /// one lookup, so the stored set a tag write changes is the very set a read reports.
    ///
    /// It spans exactly the resource kinds that read spans, because a stream and a channel
    /// each carry ONE stored tag set: whichever kind the ARN names, an assignment and an
    /// unassignment reach that very set.
    pub fn resource_tags_by_arn_mut(
        &mut self,
        now: SystemTime,
        region: &str,
        arn: &str,
    ) -> Option<&mut HashMap<String, String>> {
        // Which kind the ARN names is decided through the immutable lookups, so a write
        // addresses a resource by exactly the rule a read does; only the record it resolved
        // is then borrowed mutably.
        if self.channel_by_arn(region, arn).is_some() {
            return self
                .channel_by_arn_mut(region, arn)
                .map(|channel| &mut channel.tags);
        }
        self.streams
            .values_mut()
            .filter(|stream| stream.region == region && stream.is_held(now))
            .find(|stream| stream.arn == arn)
            .map(|stream| &mut stream.tags)
    }
}

/// Why a write places a record in no shard of the stream it addresses. The operation maps
/// it onto the error ITS model declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordPlacementFailure {
    /// The stream's records are placed by the producer's partition key, and the request
    /// carried none ([`PARTITION_KEY_REQUIRED_MESSAGE`]).
    PartitionKeyRequired,
    /// The request named an `ExplicitHashKey` outside the 128-bit key space hash keys live
    /// in ([`hash_key_out_of_range_message`]), so it names no position in any shard map at
    /// all.
    HashKeyOutOfRange(String),
    /// The hash key the request named — explicitly, or through its partition key — is inside
    /// the key space but lies in no open shard's range
    /// ([`hash_key_outside_the_shard_map_message`]).
    OutsideTheShardMap(String),
    /// Every shard of the stream is closed, so nothing can be written to it.
    NoOpenShard,
}

/// Why a request's `StreamName` / `StreamARN` pair names no stream a store holds. Each
/// operation maps it onto the error ITS model declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamLookupFailure {
    /// The request named neither member, so it names no stream at all.
    Unspecified,
    /// The request named both, and the name it gave disagrees with the one its ARN carries.
    /// Both halves travel with the failure, since the refusal names them
    /// ([`stream_identifier_mismatch_message`]).
    Mismatch {
        /// The `StreamName` the request carried.
        name: String,
        /// The `StreamARN` the request carried.
        arn: String,
    },
    /// The members are well formed, but this account and region hold no such stream.
    NotFound,
}

/// Why a request's `ConsumerARN` / `StreamARN` / `ConsumerName` members name no consumer a
/// store holds. Each operation maps it onto the error ITS model declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumerLookupFailure {
    /// The request named neither the consumer's ARN nor a stream-and-name pair, so it
    /// names no consumer at all.
    ConsumerUnspecified,
    /// The request named members that name different consumers.
    Mismatch,
    /// The members are well formed, but this account and region hold no such stream.
    StreamNotFound,
    /// The stream is held, but no consumer of it answers to the request's members.
    ConsumerNotFound,
}

/// How a request that names no stream is refused: the rule `DescribeStream` publishes
/// ("you must use either the StreamARN or the StreamName parameter, or both"), which is
/// what makes the refusal part of the contract rather than wording, worded the way the
/// service words it for a request carrying neither identifier — one sentence of fixed
/// text, with nothing taken from the request.
pub const STREAM_IDENTIFIER_REQUIRED_MESSAGE: &str =
    "Stream arn and stream name can't be empty at the same time";

/// How an operation whose ONLY stream identifier is `StreamARN` refuses a request that
/// names no stream.
///
/// The SAME rule [`STREAM_IDENTIFIER_REQUIRED_MESSAGE`] states ("you must use either the
/// StreamARN or the StreamName parameter, or both", `DescribeStream`), reduced to the one
/// parameter such an operation has: its input carries no `StreamName`, and `StreamId` is
/// documented "Not Implemented. Reserved for future use", so `StreamARN` is the only member
/// that can name a stream at all.
pub const STREAM_ARN_REQUIRED_MESSAGE: &str = "you must use the StreamARN parameter";

/// How a request whose `StreamName` disagrees with its `StreamARN` is refused, worded the
/// way this service words that error class: "Input stream name `<streamName>` doesn't match
/// the inferred name in stream arn `<nameFromArn>`".
///
/// Built FROM the request: the `StreamName` it carried, and the stream NAME its `StreamARN`
/// carries in the `stream/<name>` resource segment ([`stream_name_in`]). Those two are what
/// the pair is reconciled against each other — "you must use either the StreamARN or the
/// StreamName parameter, or both" (`DescribeStream`), so a pair that AGREES is served and
/// only one that disagrees is refused — which is why the refusal names BOTH halves rather
/// than asking the caller to drop one of them.
pub fn stream_identifier_mismatch_message(stream_name: &str, stream_arn: &str) -> String {
    format!(
        "Input stream name {stream_name} doesn't match the inferred name in stream arn {}",
        stream_name_in(stream_arn)
    )
}

/// How an operation states that this account and region hold no stream under the identifier
/// the request named, worded the way this service words that error class: "Stream `<name>`
/// under account `<account>` not found."
///
/// Built FROM the request and the emulator's own account: the variable part is the stream
/// NAME at fault, which an identifier that is a `StreamARN` carries in its resource segment
/// ([`stream_name_in`]) and one that is a `StreamName` already is.
pub fn stream_not_found_message(identifier: &str) -> String {
    format!(
        "Stream {} under account {} not found.",
        stream_name_in(identifier),
        ::engine::emulator_account_id()
    )
}

/// How an operation that distinguishes two streams of one name by the instant they were
/// created states that this account and region hold no stream of that name created THEN:
/// "Stream `<name>` under account `<account>` with creation time `<timestamp>` not found."
///
/// The same error class [`stream_not_found_message`] words, for the one member that narrows
/// the lookup past the name — "Specify this input parameter to distinguish data streams that
/// have the same name" (`ListShardsInput$StreamCreationTimestamp`) — so the sentence carries
/// the instant the request asked for beside the name and the account. Built FROM the request:
/// the identifier it addressed the stream by, and the instant it supplied taken at its WHOLE
/// SECONDS (`asked_secs`) and spelled the way the wire spells a modelled `Timestamp`
/// ([`instant_at`]).
///
/// Second precision is what the sentence states, not a rounding of this crate's choosing: a
/// read of a stream reports its creation instant with a finer fraction, so that is the value a
/// client sends back here, and the recorded answer to such a request spells the creation time
/// to the second. Taking the instant's seconds is also what keeps the sentence an instant at
/// all for a value no finer unit could hold.
pub fn stream_creation_time_not_found_message(identifier: &str, asked_secs: i64) -> String {
    format!(
        "Stream {} under account {} with creation time {} not found.",
        stream_name_in(identifier),
        ::engine::emulator_account_id(),
        instant_at(asked_secs.saturating_mul(1_000))
    )
}

/// The stream NAME `identifier` names: a `StreamARN` names its stream by name, in the
/// resource segment [`stream_arn`] builds as `stream/<name>`, and a `StreamName` is that
/// name already.
fn stream_name_in(identifier: &str) -> &str {
    identifier
        .rsplit_once("stream/")
        .map_or(identifier, |(_, name)| name)
}

/// How an operation that needs a ready stream refuses one that is not ready yet: the rule
/// `ResourceInUseException` publishes ("The resource is not available for this operation.
/// For successful operation, the resource must be in the ACTIVE state"), worded the way the
/// service words that refusal — the stream named by the `StreamName` it is stored under, the
/// account that holds it, and the state it is in INSTEAD of `ACTIVE`.
///
/// Built from the STORED record rather than the request, the same way
/// [`stream_on_demand_message`] is: a stream addressed by its ARN is refused with the same
/// sentence as one addressed by its name, since the stream a refusal names is the stored one
/// either way.
///
/// ONE sentence for the one rule, whichever modelled error carries it: an operation that has
/// no shard map to serve states the same thing through its not-found error as one that
/// refuses the change through its in-use error.
pub fn stream_not_active_message(stream_name: &str, status: &str) -> String {
    format!(
        "Stream {stream_name} under account {} not {STREAM_ACTIVE}, instead in state {status}",
        ::engine::emulator_account_id()
    )
}

/// How an operation that serves a stream while a change of it is in flight refuses one that
/// is not ready at all: the SAME rule [`stream_not_active_message`] states
/// (`ResourceInUseException`, "For successful operation, the resource must be in the ACTIVE
/// state"), for an operation whose serving states are `ACTIVE` and `UPDATING` both ("You can
/// continue to read and write data to your stream while its status is UPDATING",
/// `UpdateShardCount`) — so the sentence names BOTH states the stream is not in.
///
/// Built from the STORED record the same way [`stream_not_active_message`] is: the stream
/// named by the `StreamName` it is stored under, the account that holds it, and the state it
/// is in instead.
pub fn stream_not_active_or_updating_message(stream_name: &str, status: &str) -> String {
    format!(
        "Stream {stream_name} under account {} not {STREAM_ACTIVE} or {STREAM_UPDATING}, \
         instead in state {status}",
        ::engine::emulator_account_id()
    )
}

/// How disabling server-side encryption refuses a stream that is not encrypted: there is
/// no encryption on it to disable, which is the modelled "a specified parameter ... is not
/// supported, or can't be used" refusal. FIXED text for this refusal — it names no stream
/// and carries no request- or state-derived value.
pub const UNENCRYPTED_STREAM_MESSAGE: &str = "Cannot stop encrypting an unencrypted stream.";

/// How an encryption operation refuses an `EncryptionType` it does not serve: "The
/// encryption type. The only valid value is KMS"
/// (`StopStreamEncryptionInput$EncryptionType`), so the value at fault names the refusal.
pub fn encryption_type_unsupported_message(encryption_type: &str) -> String {
    format!(
        "EncryptionType {encryption_type} is not supported; the only valid value is \
         {ENCRYPTION_TYPE_KMS}"
    )
}

/// How an encryption change refuses a stream that has used up the rolling window's changes
/// of that kind: "You can successfully disable server-side encryption 25 times in a rolling
/// 24-hour period" (`StopStreamEncryption`), and the same ceiling for applying a key
/// (`StartStreamEncryption`). Built FROM the request and the stored record, so the stream
/// and the count at fault name it.
///
/// ONE sentence for the one rule: both encryption operations are bounded by one ceiling
/// over one window, so they refuse a stream past it identically rather than each wording it.
pub fn encryption_change_limit_message(identifier: &str, changed: usize) -> String {
    format!(
        "stream {identifier} has had its server-side encryption changed {changed} times in \
         the last 24 hours; you can successfully change server-side encryption \
         {MAX_ENCRYPTION_CHANGES_PER_WINDOW} times in a rolling 24-hour period"
    )
}

/// How many of `instants` fall inside the window ending at `now`, which is what
/// [`MAX_ENCRYPTION_CHANGES_PER_WINDOW`] bounds.
fn encryption_changes_in_window(instants: &[SystemTime], now: SystemTime) -> usize {
    let from = now
        .checked_sub(ENCRYPTION_CHANGE_WINDOW)
        .unwrap_or(UNIX_EPOCH);
    instants.iter().filter(|at| **at >= from).count()
}

/// Record an encryption change at `at`, dropping the ones the window has rolled past, so
/// the list holds at most the changes the ceiling is counted over.
fn record_encryption_change(instants: &mut Vec<SystemTime>, at: SystemTime) {
    let from = at
        .checked_sub(ENCRYPTION_CHANGE_WINDOW)
        .unwrap_or(UNIX_EPOCH);
    instants.retain(|instant| *instant >= from);
    instants.push(at);
}

/// How `CreateStream` refuses a request that names BOTH the on-demand capacity mode and a
/// `ShardCount`: "Kinesis Data Streams automatically manages the shards" of an on-demand
/// stream, so the member is not one such a create may carry. One sentence of fixed text,
/// with nothing taken from the request \u2014 the member and the mode it is refused for are the
/// only two things the refusal is about, and both are the same every time.
pub const ON_DEMAND_SHARD_COUNT_MESSAGE: &str =
    "ShardCount cannot be set while creating stream in On-Demand StreamMode";

/// How an operation that only serves a provisioned stream refuses one whose shard count
/// the service manages itself: "specifies that you tried to invoke this API for a data
/// stream with the on-demand capacity mode. This API is only supported for data streams
/// with the provisioned capacity mode" (`ValidationException`). Built FROM the stored
/// record and this account, so the stream at fault names it.
///
/// ONE sentence for the one rule: `UpdateShardCount`, `SplitShard` and `MergeShards` all
/// re-tile the shard map a provisioned stream owns, so they refuse an on-demand stream
/// identically rather than each wording it.
pub fn stream_on_demand_message(stream_name: &str) -> String {
    format!(
        "Request is invalid. Stream {stream_name} under account {} is in On-Demand mode.",
        ::engine::emulator_account_id()
    )
}

/// How an operation that only serves an on-demand stream refuses one whose shard count the
/// caller owns: "Provisioned capacity mode streams do not support warm throughput
/// configuration" (`UpdateStreamWarmThroughput`). Built FROM the stored record and this
/// account, so the stream at fault names it.
///
/// The MIRROR of [`stream_on_demand_message`], worded the same way for the same rule read the
/// other way round: one sentence for every operation a stream's capacity mode excludes.
pub fn stream_provisioned_message(stream_name: &str) -> String {
    format!(
        "Request is invalid. Stream {stream_name} under account {} is in Provisioned mode.",
        ::engine::emulator_account_id()
    )
}

/// How an operation refuses a warm throughput past the ceiling one stream may be scaled to:
/// "By default, you cannot do the following: Scale to more than 10 GiBps for an on-demand
/// stream... To request an increase in the call rate limit, the shard limit for this API, or
/// your overall shard limit, use the limits form" (`UpdateStreamWarmThroughput`), which is the
/// modelled "the requested resource exceeds the maximum number allowed" refusal.
///
/// Built FROM the request, this account and the published ceiling
/// ([`MAX_WARM_THROUGHPUT_MI_BPS`]): the value asked for and the one it passes, so a caller
/// can compute a target that fits from the refusal alone.
pub fn warm_throughput_over_ceiling_message(requested: i32) -> String {
    format!(
        "This request would exceed the warm throughput limit for the account {}. Requested \
         warm throughput: {requested} MiBps. Limit: {MAX_WARM_THROUGHPUT_MI_BPS} MiBps. Refer \
         to the AWS Service Limits page \
         (http://docs.aws.amazon.com/general/latest/gr/aws_service_limits.html) for current \
         limits and how to request higher limits.",
        ::engine::emulator_account_id()
    )
}

/// How a request that would take an account and region past its shard quota is refused —
/// the message AWS answers `LimitExceededException` with for a request that would "create
/// more shards than are authorized for your account" (`CreateStream`) or "scale up to more
/// than the shard limit for your account" (`UpdateShardCount`).
///
/// Built FROM the request and the store: the account this emulator serves, the region the
/// request was signed for, the usage and the quota `DescribeLimits` reports at that instant
/// ([`ComAmazonawsKinesisKinesis20131202State::shard_quota_in`]), and the shards the
/// refused request would have laid
/// down — so a caller can compute a size that fits from the refusal alone.
///
/// ONE sentence for the one rule: `CreateStream`, `UpdateShardCount`, `SplitShard` and
/// `MergeShards` all open shards against that one quota, so they refuse it identically
/// rather than each wording it.
pub fn shard_limit_exceeded_message(
    region: &str,
    open_shard_count: i32,
    shard_limit: i32,
    additional_shards: i32,
) -> String {
    format!(
        "This request would exceed the shard limit for the account {} in {region}. Current \
         shard count for the account: {open_shard_count}. Limit: {shard_limit}. Number of \
         additional shards that would have resulted from this request: {additional_shards}. \
         Refer to the AWS Service Limits page \
         (http://docs.aws.amazon.com/general/latest/gr/aws_service_limits.html) for current \
         limits and how to request higher limits.",
        ::engine::emulator_account_id()
    )
}

/// How a request that would leave a region holding more streams of the on-demand capacity
/// mode than it may ([`MAX_ON_DEMAND_STREAMS_PER_REGION`]) is refused. Built FROM the
/// store, so the count the ceiling was taken against names it.
///
/// ONE sentence for the one rule: a stream takes that capacity mode at a create and at a
/// mode change, so both refuse the ceiling identically rather than each wording it — the
/// same shape as the per-region ceilings `CreateStream` is refused past.
pub fn on_demand_stream_ceiling_exceeded_message(held: usize) -> String {
    format!(
        "this region holds {held} streams with the {STREAM_MODE_ON_DEMAND} capacity mode, \
         and one more would exceed the {MAX_ON_DEMAND_STREAMS_PER_REGION} on-demand streams \
         authorized for this account and region"
    )
}

/// How `UpdateShardCount` refuses a target below half the stream's current open shard
/// count: "you cannot do the following: ... scale down below half your current shard count
/// for a stream" (`UpdateShardCount`). Built FROM the stored record and the request, so the
/// count the bound was taken against and the target at fault both name it.
pub fn scale_down_below_half_message(open_shard_count: i32, target_shard_count: i32) -> String {
    format!(
        "UpdateShardCount cannot scale down below half your current open shard count. \
         Current open shard count: {open_shard_count} Target shard count: {target_shard_count}"
    )
}

/// How `UpdateShardCount` refuses a target over double the stream's current open shard
/// count: "you cannot do the following: ... scale up to more than double your current shard
/// count for a stream" (`UpdateShardCount`). Built FROM the stored record and the request,
/// so the count the bound was taken against and the target at fault both name it.
pub fn scale_up_over_double_message(open_shard_count: i32, target_shard_count: i32) -> String {
    format!(
        "UpdateShardCount cannot scale up over double your current open shard count. \
         Current open shard count: {open_shard_count} Target shard count: {target_shard_count}"
    )
}

/// How a retention change refuses a window past the longest one a stream may carry
/// ([`MAX_RETENTION_PERIOD_HOURS`]). Built FROM the request, so the window at fault names
/// it.
///
/// ONE sentence for the one rule: both retention operations carry the same member against
/// the same ceiling, so they refuse it identically rather than each wording it.
pub fn retention_period_too_long_message(requested_hours: i32) -> String {
    format!(
        "Maximum allowed retention period is {MAX_RETENTION_PERIOD_HOURS} hours. Requested \
         retention period ({requested_hours} hours) is too long."
    )
}

/// How a retention change refuses a window below the shortest one a stream may carry
/// ([`MIN_RETENTION_PERIOD_HOURS`]). The counterpart of
/// [`retention_period_too_long_message`], for the other end of the same window.
pub fn retention_period_too_short_message(requested_hours: i32) -> String {
    format!(
        "Minimum allowed retention period is {MIN_RETENTION_PERIOD_HOURS} hours. Requested \
         retention period ({requested_hours} hours) is too short."
    )
}

/// How `IncreaseStreamRetentionPeriod` refuses a window shorter than the one the stream
/// already carries: its member is "the new retention period of the stream, in hours. Must
/// be more than the current retention period"
/// (`IncreaseStreamRetentionPeriodInput$RetentionPeriodHours`), and shortening a window is
/// the other operation's to serve, which the sentence names. Built FROM the request and the
/// stored record, so the window at fault and the one the stream carries both name it.
pub fn retention_period_not_longer_message(
    stream_identifier: &str,
    requested_hours: i32,
    current_hours: i32,
) -> String {
    format!(
        "Requested retention period ({requested_hours} hours) for stream {stream_identifier} \
         can not be shorter than existing retention period ({current_hours} hours). Use \
         DecreaseRetentionPeriod API."
    )
}

/// How `DecreaseStreamRetentionPeriod` refuses a window longer than the one the stream
/// already carries: its member is "the new retention period of the stream, in hours. Must
/// be less than the current retention period"
/// (`DecreaseStreamRetentionPeriodInput$RetentionPeriodHours`), and lengthening a window is
/// the other operation's to serve. The same sentence as
/// [`retention_period_not_longer_message`] with the direction and the operation it names
/// turned around: ONE wording for the one rule, in both directions.
pub fn retention_period_not_shorter_message(
    stream_identifier: &str,
    requested_hours: i32,
    current_hours: i32,
) -> String {
    format!(
        "Requested retention period ({requested_hours} hours) for stream {stream_identifier} \
         can not be longer than existing retention period ({current_hours} hours). Use \
         IncreaseRetentionPeriod API."
    )
}

/// How a manual reshard refuses a shard that is no longer open: a closed shard has already
/// given up its slice of the hash key space to the children a split or a merge minted, so
/// there is nothing left of it to re-tile.
///
/// ONE sentence for the one rule: `SplitShard` and `MergeShards` both re-tile OPEN shards,
/// so they refuse a closed one identically rather than each wording it.
pub fn shard_not_open_message(shard_id: &str, stream_identifier: &str) -> String {
    format!(
        "Shard {shard_id} in stream {stream_identifier} under account {} has already been \
         merged or split, and thus is not eligible for merging or splitting.",
        ::engine::emulator_account_id()
    )
}

/// How `MergeShards` refuses two shards whose ranges do not meet: "two shards are
/// considered adjacent if the union of the hash key ranges for the two shards form a
/// contiguous set with no gaps" (`MergeShards`). The fixed text is the service's own; its
/// variable parts are built FROM the request and the stored stream, so the pair at fault,
/// the stream they belong to and the account that holds it all name the refusal.
pub fn shards_not_adjacent_message(
    shard_to_merge: &str,
    adjacent_shard_to_merge: &str,
    stream_identifier: &str,
) -> String {
    format!(
        "Shards {shard_to_merge} and {adjacent_shard_to_merge} in stream {stream_identifier} \
         under account {} are not an adjacent pair of shards eligible for merging",
        ::engine::emulator_account_id()
    )
}

/// How `SplitShard` refuses a `NewStartingHashKey` that does not fall strictly inside the
/// parent shard's own range — a key that would leave one of the two children covering no
/// hash key at all. Built FROM the request and the stored shard, so the key at fault and
/// the range it had to fall inside both name it.
pub fn split_hash_key_outside_the_parent_message(
    new_starting_hash_key: &str,
    shard_id: &str,
    stream_identifier: &str,
    starting_hash_key: &str,
    ending_hash_key: &str,
) -> String {
    format!(
        "NewStartingHashKey {new_starting_hash_key} used in SplitShard() on shard {shard_id} \
         in stream {stream_identifier} under account {} is not both greater than one plus the \
         shard's StartingHashKey {starting_hash_key} and less than the shard's EndingHashKey \
         {ending_hash_key}.",
        ::engine::emulator_account_id()
    )
}

/// How an assignment that would leave a resource carrying more tags than one may carry is
/// refused: the ceiling ([`MAX_STREAM_TAGS`]) is on what the resource carries once the
/// request is served, so the refusal states the count the assignment would reach.
///
/// ONE sentence for the one rule: `AddTagsToStream` and `TagResource` are two ways into the
/// SAME stored tag set, so they refuse the ceiling identically rather than each wording it.
pub fn tag_ceiling_exceeded_message(resulting: usize) -> String {
    format!(
        "assigning these tags would leave the resource carrying {resulting} tags, past the \
         {MAX_STREAM_TAGS} tags one resource may carry"
    )
}

/// The `MetricsName` value that stands for the whole set of shard-level metrics rather
/// than for one of them: "The value \"ALL\" enables every metric"
/// (`EnableEnhancedMonitoringInput$ShardLevelMetrics`).
pub const METRICS_NAME_ALL: &str = "ALL";

/// Every shard-level metric enhanced monitoring reports, in the order the model's
/// `MetricsName` enum lists them (`smithy_query(query="shape",shape="MetricsName")`, whose
/// values this is filtered from), with [`METRICS_NAME_ALL`] left out: that value names the
/// whole catalog rather than a metric of its own.
///
/// This is the order every report of the enhanced set is in, so two reads of one stream
/// cannot disagree about it.
pub fn shard_level_metric_catalog() -> Vec<&'static str> {
    METRICS_NAME_VALUES
        .iter()
        .copied()
        .filter(|metric| *metric != METRICS_NAME_ALL)
        .collect()
}

/// The shard-level metrics a request's `ShardLevelMetrics` names, in the catalog's order
/// and without a repeat: [`METRICS_NAME_ALL`] stands for every metric, so a request naming
/// it names the whole catalog. The member targets the modelled `MetricsName` enum, whose
/// constraint the framework's own walk refuses a request against before any handler runs,
/// and a value outside the catalog names no metric here either.
pub fn requested_shard_level_metrics(requested: &[String]) -> Vec<String> {
    let names_every_metric = requested.iter().any(|metric| metric == METRICS_NAME_ALL);
    shard_level_metric_catalog()
        .into_iter()
        .filter(|metric| names_every_metric || requested.iter().any(|named| named == metric))
        .map(str::to_string)
        .collect()
}

/// How a request that names no consumer is refused: the rule `DescribeStreamConsumer`
/// publishes ("provide the ARN of the consumer. Alternatively, you can provide the ARN of
/// the data stream and the name you gave the consumer when you registered it").
pub const CONSUMER_IDENTIFIER_REQUIRED_MESSAGE: &str =
    "you must provide the ConsumerARN, or the StreamARN and the ConsumerName";

/// How a request whose consumer identifiers name different consumers is refused: "You may
/// also provide all three parameters, as long as they don't conflict with each other"
/// (`DescribeStreamConsumer`).
pub const CONSUMER_IDENTIFIER_CONFLICT_MESSAGE: &str =
    "ConsumerARN, StreamARN and ConsumerName name different consumers; they must not \
     conflict with each other";

/// How a refusal states that no consumer the request's members name is registered with the
/// stream they name: the consumer at fault, then the stream and the account it was looked
/// for under, then that it was not found.
///
/// Built FROM the request and from this emulator's account, so the consumer is named the way
/// the request named it and the stream is the one it addressed, by the NAME the addressing
/// ARN carries ([`stream_name_in_arn`]) — a request that resolves by `ConsumerARN` alone
/// still names a stream, since a consumer's ARN is its stream's ARN extended.
pub fn consumer_not_found_message(consumer_identifier: &str, stream_identifier: &str) -> String {
    let stream_name = stream_name_in_arn(stream_identifier)
        .or_else(|| stream_name_in_arn(consumer_identifier))
        .unwrap_or(stream_identifier);
    format!(
        "Consumer {consumer_identifier} under stream: {stream_name}, account {} not found.",
        ::engine::emulator_account_id()
    )
}

/// The stream's NAME as a `StreamARN` or a `ConsumerARN` carries it: the segment after
/// `stream/`, up to the `/consumer/` a consumer's ARN continues with. The modelled patterns
/// are `^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$` and
/// `^(arn):aws.*:kinesis:.*:\d{12}:.*stream\/[a-zA-Z0-9_.-]+\/consumer\/[a-zA-Z0-9_.-]+:[0-9]+$`,
/// and a stream name carries no `/` (`^[a-zA-Z0-9_.-]+$`), so the first segment is the whole
/// name. `None` for an identifier carrying no such segment.
pub(crate) fn stream_name_in_arn(identifier: &str) -> Option<&str> {
    let resource = identifier.split_once("stream/")?.1;
    Some(resource.split('/').next().unwrap_or(resource))
}

/// How a registration refuses a name the stream already carries a consumer under: "For a
/// given Kinesis data stream, each consumer must have a unique name"
/// (`RegisterStreamConsumerInput$ConsumerName`), which is the modelled "resource is not
/// available for this operation" refusal. Built FROM the request and the stored record, so
/// the consumer, the stream and the account at fault are what name it.
pub fn consumer_already_exists_message(consumer_name: &str, stream_name: &str) -> String {
    format!(
        "Consumer {consumer_name} under stream {stream_name} already exists for account {}.",
        ::engine::emulator_account_id()
    )
}

/// How a registration refuses a stream that already carries every consumer it may carry:
/// "you can register up to 20 consumers per stream to use Enhanced Fan-out"
/// (`RegisterStreamConsumer`). Built FROM the stored record, so the stream and the count at
/// fault name it.
pub fn consumer_ceiling_exceeded_message(stream_identifier: &str, registered: usize) -> String {
    format!(
        "stream {stream_identifier} already has {registered} registered consumers; you can \
         register up to {MAX_CONSUMERS_PER_STREAM} consumers per stream to use Enhanced \
         Fan-out"
    )
}

/// How a registration refuses a stream that already has as many consumers being created as
/// may be created at once: "you cannot have more than 5 consumers in a CREATING status at
/// the same time. Registering a 6th consumer while there are 5 in a CREATING status results
/// in a LimitExceededException" (`RegisterStreamConsumer`).
pub fn consumers_creating_message(stream_identifier: &str) -> String {
    format!(
        "stream {stream_identifier} already has {MAX_CONSUMERS_CREATING} consumers in \
         {CONSUMER_CREATING} status; you cannot have more than \
         {MAX_CONSUMERS_CREATING} consumers in a {CONSUMER_CREATING} status at the same time"
    )
}

/// How a delete refuses a stream that still carries consumers: "If this parameter is unset
/// (null) or if you set it to false, and the stream has registered consumers, the call to
/// DeleteStream fails with a ResourceInUseException"
/// (`DeleteStreamInput$EnforceConsumerDeletion`), so the refusal names the member that
/// serves the call anyway. Built FROM the stored record and this account, so the stream
/// standing in the way is what names it.
pub fn stream_has_consumers_message(stream_name: &str) -> String {
    format!(
        "DeleteStream can\u{2019}t delete {}:{stream_name} because there are consumers \
         associated with this data stream. To delete {stream_name} anyway, set the \
         EnforceConsumerDeletion parameter to true to force the deletion of all the \
         consumers associated with this data stream.",
        ::engine::emulator_account_id()
    )
}

/// How a delete refuses a stream a channel still sources: a stream named in a channel's
/// `StreamConfigurationList` is a dependency of that channel, so the channels have to go
/// first — "For successful operation, the resource must be in the ACTIVE state"
/// (`ResourceInUseException`), which a stream another resource depends on is not available
/// for. The sentence is the one the recorded refusal of this call carries, which names the
/// dependency class and what to do about it; built FROM the request and this account, so the
/// stream standing in the way and the account holding it are what fill it.
pub fn stream_has_channels_message(stream_name: &str) -> String {
    format!(
        "Stream {stream_name} under account {} cannot be deleted because it is associated \
         with one or more channels. Delete all associated channels before deleting the \
         stream.",
        ::engine::emulator_account_id()
    )
}

/// The ARN a consumer named `name`, registered at `created_at` with the stream `stream_arn`
/// addresses, is addressed by.
///
/// "If you delete a consumer and then create a new one with the same name, it won't have
/// the same ARN. That's because consumer ARNs contain the creation timestamp"
/// (`Consumer$ConsumerARN`), so the registration instant is the ARN's last segment — in
/// whole seconds since the epoch, as the modelled `ConsumerARN` pattern
/// (`…stream/<name>/consumer/<name>:[0-9]+`) ends.
pub fn consumer_arn(stream_arn: &str, name: &str, created_at: SystemTime) -> String {
    format!("{stream_arn}/consumer/{name}:{}", epoch_seconds(created_at))
}

/// Whole seconds since the epoch at `at`, which is the segment a [`consumer_arn`] ends
/// with.
pub fn epoch_seconds(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// How a create refuses a request that names no destination, or names both: "You must
/// specify either S3DestinationConfiguration or S3TablesDestinationConfiguration, but not
/// both" (`CreateChannel`). FIXED text for the one rule both halves break — it names no
/// request- or state-derived value.
pub const CHANNEL_DESTINATION_REQUIRED_MESSAGE: &str =
    "You must specify either S3DestinationConfiguration or S3TablesDestinationConfiguration, \
     but not both.";

/// The channel name an identifier carries: a `ChannelARN` ends in the resource
/// `channel/<name>` (pattern `^arn:aws.*:kinesis:.*:\d{12}:channel/\S+$`) and a `ChannelName`
/// carries no `/` (pattern `^[a-zA-Z0-9_.-]+$`), so the name is what follows the last one —
/// and an identifier that carries none is already a bare name, answered as it arrived.
pub fn channel_name_in(identifier: &str) -> &str {
    identifier
        .rsplit_once('/')
        .map_or(identifier, |(_, name)| name)
}

/// How an operation states that this account holds no channel under the identifier the
/// request named. Built FROM the request and the account serving it: the channel is named the
/// way a client addresses it — by its NAME, which is what the `ChannelARN` carries as its
/// resource ([`channel_name_in`]) — and never by anything else the store happens to hold.
pub fn channel_not_found_message(identifier: &str) -> String {
    format!(
        "Channel {} under account {} not found.",
        channel_name_in(identifier),
        ::engine::emulator_account_id()
    )
}

/// How a read states that this account and region hold no Kinesis resource under the ARN
/// the request named, for an operation whose `ResourceARN` addresses a resource of ANY kind
/// it may name (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`, which a `ConsumerARN`
/// satisfies too): the sentence this service already states for the kind that ARN names, so
/// one ARN is refused in one wording whatever operation it reached.
pub fn resource_not_found_message(arn: &str) -> String {
    if arn_names_channel(arn) {
        channel_not_found_message(arn)
    } else if arn_names_consumer(arn) {
        // A consumer's ARN is its stream's ARN extended (`…stream/<name>/consumer/<name>:<ts>`),
        // so the thing not found is the CONSUMER that ARN names, under the stream it extends.
        consumer_not_found_message(arn, arn)
    } else {
        stream_not_found_message(arn)
    }
}

/// Whether `arn`'s resource segment names a CONSUMER registered with a stream, which a
/// `ResourceARN` may address too: a `ConsumerARN` continues its stream's resource segment
/// with `/consumer/<name>:<timestamp>` ([`consumer_arn`] builds it).
fn arn_names_consumer(arn: &str) -> bool {
    stream_name_in_arn(arn).is_some_and(|_| arn.contains("/consumer/"))
}

/// The most bytes of policy document this emulator retains for one resource. A CHOSEN
/// ceiling, not an AWS-documented limit (`Policy` carries no modelled `@length`): one
/// document is kept per resource for the life of the process, so what a single request may
/// make the store grow by is bounded, and a document past it is refused rather than stored
/// ([`policy_too_large_message`]).
pub const MAX_RESOURCE_POLICY_BYTES: usize = 20 * 1024;

/// How an attach refuses a document larger than this emulator retains: the modelled "A
/// specified parameter exceeds its restrictions" refusal (`InvalidArgumentException`). Built
/// FROM the request, so the size at fault and the ceiling it exceeded name it.
pub fn policy_too_large_message(sent: usize) -> String {
    format!(
        "Policy must be at most {MAX_RESOURCE_POLICY_BYTES} bytes; the document sent is \
         {sent} bytes."
    )
}

/// How an attach refuses a `Policy` that is no policy document at all: a resource-based
/// policy "is formatted as a JSON string" (`PutResourcePolicyInput$Policy`) and names "the
/// identity of the principal and the actions allowed on this resource", which is a JSON
/// object — anything else is the modelled "a specified parameter ... is not supported, or
/// can't be used" refusal, stated as the service's policy-document validation states it.
///
/// FIXED text: the refusal quotes neither the document, nor an offset, nor a member name, so
/// a document that does not parse and one that parses as something other than a policy
/// object are refused with the one sentence.
pub const POLICY_INVALID_JSON_MESSAGE: &str =
    "Policy validation error: This policy contains invalid Json";

/// What a read reports for a resource that carries no resource-based policy: a policy "is
/// formatted as a JSON string" (`GetResourcePolicyOutput$Policy`) holding "the identity of
/// the principal and the actions allowed on this resource", so a resource no statement
/// governs is reported as the JSON object carrying no statement at all. It is never stored:
/// the stored slot stays empty until an attach writes a document into it.
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// How a delete states that the resource the request named carries no policy to
/// remove, worded with the fixed tail this service states every not-found with
/// ("... under account `<account>` not found."): the thing not found is the POLICY of that
/// resource, and the resource is named the way the request addressed it — by its
/// `ResourceARN`, the only identifier these operations have.
pub fn resource_policy_not_found_message(arn: &str) -> String {
    format!(
        "Policy for resource {arn} under account {} not found.",
        ::engine::emulator_account_id()
    )
}

/// Whether `arn`'s resource segment names a CHANNEL, which is the other resource kind a
/// `ResourceARN` may address. An ARN carries its resource kind in the segment after the last
/// `:` ([`channel_arn`] builds it as `channel/<name>`).
fn arn_names_channel(arn: &str) -> bool {
    arn.rsplit(':')
        .next()
        .is_some_and(|resource| resource.starts_with("channel/"))
}

/// How a create states that this account and region already hold a channel under the name
/// the request named: "The name is unique within your Amazon Web Services account and
/// Amazon Web Services Region" (`CreateChannelInput$ChannelName`). Built FROM the request
/// and the account serving it — the name at fault and the account that already holds a
/// channel under it — with the fixed spans this service states every "already exists" with.
pub fn channel_already_exists_message(channel_name: &str) -> String {
    format!(
        "Channel {channel_name} under account {} already exists.",
        ::engine::emulator_account_id()
    )
}

/// How an operation that needs a ready channel refuses one that is not ready yet: the rule
/// `ResourceInUseException` publishes ("The resource is not available for this operation. For
/// successful operation, the resource must be in the ACTIVE state"), worded the way the
/// service words that refusal for a channel — the channel named by the `ChannelName` it is
/// stored under, the account that holds it, and the state it is in INSTEAD of `ACTIVE`.
///
/// Built from the STORED record rather than the request, the same way
/// [`stream_not_active_message`] is: a channel is refused by the name it is stored under
/// whichever identifier the request addressed it by.
pub fn channel_not_active_message(channel_name: &str, status: &str) -> String {
    format!(
        "Channel {channel_name} under account {} not {CHANNEL_ACTIVE}, instead in state \
         {status}.",
        ::engine::emulator_account_id()
    )
}

/// How an update refuses a request naming the destination the channel does NOT deliver to:
/// "You cannot change the destination, source stream, record format, schema, encryption
/// configuration, or service execution role of an existing channel" (`UpdateChannel`). Built
/// FROM the request and the stored record, so the channel and the destination it delivers to
/// name it.
pub fn channel_destination_not_updatable_message(
    identifier: &str,
    destination_type: &str,
) -> String {
    format!(
        "channel {identifier} delivers to the {destination_type} destination; you cannot \
         change the destination of an existing channel"
    )
}

/// How an update refuses a request that names nothing to update: an update "Updates the data
/// freshness interval or the Amazon CloudWatch Logs configuration of an existing channel"
/// (`UpdateChannel`), so one of the three updatable members has to be there. FIXED text for
/// this refusal — it carries no request- or state-derived value.
pub const CHANNEL_UPDATE_REQUIRED_MESSAGE: &str =
    "Specify at least one of S3DestinationConfiguration, S3TablesDestinationConfiguration or \
     LoggingConfiguration to update.";

/// How a create refuses a `StreamConfigurationList` naming more source streams than one
/// channel carries: "Currently, one stream is supported per channel"
/// (`CreateChannelInput$StreamConfigurationList`). Built FROM the request, so the count at
/// fault names it.
pub fn one_stream_per_channel_message(requested: usize) -> String {
    format!(
        "StreamConfigurationList names {requested} source streams; currently \
         {MAX_STREAMS_PER_CHANNEL} stream is supported per channel"
    )
}

/// How a create refuses a source stream whose capacity the caller provisions: "This
/// operation is only supported for data streams with the on-demand capacity mode"
/// (`CreateChannel`). Built FROM the request and the stored record, so the stream and the
/// mode at fault name it.
pub fn stream_not_on_demand_message(identifier: &str, stream_mode: &str) -> String {
    format!(
        "stream {identifier} is in {stream_mode} mode; a channel is only supported for data \
         streams with the {STREAM_MODE_ON_DEMAND} capacity mode"
    )
}

/// How a create refuses a record format the channel's destination does not admit: the
/// catalog `RecordConfiguration$RecordFormatType` publishes per destination
/// ([`S3_RECORD_FORMAT_TYPES`], [`S3_TABLES_RECORD_FORMAT_TYPES`]). Built FROM the request
/// and the destination it named, so the pair at fault names it.
pub fn record_format_unsupported_message(
    record_format_type: &str,
    destination_type: &str,
) -> String {
    format!(
        "RecordFormatType {record_format_type} is not supported for a {destination_type} \
         destination; the supported values are {}",
        record_format_types_for(destination_type).join(" | ")
    )
}

/// The record formats `destination_type` admits, which is what a create checks a source
/// stream's `RecordFormatType` against.
pub fn record_format_types_for(destination_type: &str) -> &'static [&'static str] {
    if destination_type == CHANNEL_DESTINATION_S3_TABLES {
        S3_TABLES_RECORD_FORMAT_TYPES
    } else {
        S3_RECORD_FORMAT_TYPES
    }
}

/// How a create refuses a streaming table destination whose source stream names no schema:
/// `GSRSchemaARN` is "Required when the channel destination is a streaming table"
/// (`RecordConfiguration$GSRSchemaARN`). FIXED text for this refusal — it carries no
/// request- or state-derived value.
pub const GSR_SCHEMA_REQUIRED_MESSAGE: &str =
    "RecordConfiguration.GSRSchemaARN is required when the channel destination is a \
     streaming table.";

/// How a create refuses a `KeyId` that is shaped like no KMS key identifier: the forms one
/// may take are "a globally unique identifier, a fully specified ARN to either an alias or
/// a key, or an alias name prefixed by 'alias/'" (`StartStreamEncryptionInput$KeyId`).
/// Built FROM the request, so the value at fault names it.
pub fn invalid_kms_key_message(key_id: &str) -> String {
    format!(
        "Invalid KMS key identifier {key_id}; the KMS key must be a key ARN, an alias ARN, \
         a globally unique key ID or an alias name prefixed by {KMS_ALIAS_PREFIX}"
    )
}

/// How a create refuses the KMS key this service owns: "You cannot use the Amazon Kinesis
/// Data Streams service key (aws/kinesis)" (`ChannelEncryptionConfiguration$KeyId`). FIXED
/// text for this refusal — the key it names is the one key no channel may use.
pub const SERVICE_KMS_KEY_MESSAGE: &str =
    "You cannot use the Amazon Kinesis Data Streams service key (aws/kinesis); the KMS key \
     must be a customer managed key.";

/// How a create refuses a channel this emulator has no room for: the ceilings
/// ([`MAX_CHANNELS_PER_REGION`], [`MAX_CHANNELS_TOTAL`]) are on what the store holds once
/// the request is served, so the refusal states what it already holds.
pub fn channel_ceiling_exceeded_message(held_in_region: usize, held: usize) -> String {
    format!(
        "this region holds {held_in_region} channels and this emulator {held}, and another \
         would exceed the {MAX_CHANNELS_PER_REGION} channels one region and the \
         {MAX_CHANNELS_TOTAL} channels this emulator may hold; delete one before creating \
         another"
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisKinesis20131202State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisKinesis20131202State>)
}

/// The region a request was signed for, or [`DEFAULT_REGION`] when it carries no SigV4
/// credential scope or one whose region cannot name an AWS region.
///
/// The region is a store key and a quota partition, so a value that is not shaped like a
/// region name is not adopted as either: it falls back to the default region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_name(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The most characters a region name carries, comfortably past every published AWS region.
const MAX_REGION_NAME_LENGTH: usize = 32;

/// Whether `region` is shaped like an AWS region name — lowercase letters, digits and
/// dashes, bounded in length — so it can serve as a store key and a quota partition.
fn is_region_name(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_NAME_LENGTH
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A stored tag set as the modelled `Tag` list, ordered by key.
///
/// A tag list is paged through by key ("The key to use as the starting point for the list of
/// tags... gets all tags that occur after ExclusiveStartTagKey",
/// `ListTagsForStreamInput$ExclusiveStartTagKey`), so the tags a read reports need ONE total
/// order that every read agrees on, and the cursor names the key: the order is by key. ONE
/// projection for every resource kind that carries tags, so no two reads of one service
/// order or shape them differently.
pub fn tags_ordered(tags: &HashMap<String, String>) -> Vec<Tag> {
    let mut tags: Vec<Tag> = tags
        .iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            // `Tag$Value` is "An optional string", and a tag assigned a value carries it
            // however short it is — `TagValue` is modelled `len 0..256`.
            value: Some(value.clone()),
        })
        .collect();
    tags.sort_by(|left, right| left.key.cmp(&right.key));
    tags
}

/// How many tags a stored tag set would hold once `requested` were assigned to it, which is
/// what [`MAX_STREAM_TAGS`] bounds: an assignment overwrites a key the set already carries
/// rather than adding a second entry for it.
///
/// ONE reading for every resource kind that carries tags, so the ceiling a stream is held to
/// is the ceiling a channel is held to.
pub fn tag_count_with(
    tags: &HashMap<String, String>,
    requested: &HashMap<String, String>,
) -> usize {
    tags.len()
        + requested
            .keys()
            .filter(|key| !tags.contains_key(key.as_str()))
            .count()
}

/// Unassign the tags `keys` names from a stored tag set: "Removed tags are deleted and can't
/// be recovered after this operation completes successfully" (`UntagResource`), and "If you
/// specify a tag that does not exist, it is ignored" (`RemoveTagsFromStream`), so a key the
/// resource does not carry removes nothing and every tag the request does not name stays as
/// it was.
///
/// ONE rule for every resource kind that carries tags, so no two removals of one service
/// delete differently ([`Stream::unassign_tags`] is this function over a stream's own set).
pub fn unassign_tags<'keys>(
    tags: &mut HashMap<String, String>,
    keys: impl IntoIterator<Item = &'keys str>,
) {
    for key in keys {
        tags.remove(key);
    }
}

/// The modelled timestamp for a stored instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => ::aws_smithy_types::DateTime::from_secs_f64(since.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The ARN a stream named `name` in `region` is addressed by, built through the shared
/// `engine` helper so the account segment is the one canonical placeholder account.
pub fn stream_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SERVICE_ARN_NAMESPACE, region, &format!("stream/{name}"))
}

/// The ARN a channel named `name` in `region` is addressed by, built through the same
/// shared `engine` helper, inside the `ChannelARN` pattern
/// (`^arn:aws.*:kinesis:.*:\d{12}:channel/\S+$`).
pub fn channel_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(SERVICE_ARN_NAMESPACE, region, &format!("channel/{name}"))
}

/// The `ChannelId` of the channel `region` holds under `name`, created at `created_at`:
/// "The unique identifier of the channel" (`ChannelDescription$ChannelId`), inside its
/// modelled `@length` of 1..64.
///
/// An id is opaque to a client, so this one is DERIVED from what identifies the channel —
/// its region, its name and the instant it was created at — through a digest crate: two
/// channels differ, a name re-created after a delete differs from the channel it replaces,
/// and the value a read reports never changes. Nothing here authenticates or protects
/// anything; the digest is only how the three parts are folded into one short id.
pub fn channel_id(region: &str, name: &str, created_at: SystemTime) -> String {
    let seed = format!("{region}/{name}/{}", epoch_millis(created_at));
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(seed.as_bytes());
    // Half the digest, which is inside the modelled `ChannelId` `@length` of 1..64 and
    // still far past the collision range of the channels one emulator holds.
    ::hex::encode(&digest[..CHANNEL_ID_DIGEST_BYTES])
}

/// How many bytes of the digest a `ChannelId` carries, hex-encoded — 32 characters, inside
/// the member's modelled `@length` of 1..64.
const CHANNEL_ID_DIGEST_BYTES: usize = 16;

/// The CloudWatch Logs log group a channel logs to when the request names none: "Defaults
/// to /aws/kinesis/{channelName}/{channelId}" (`CloudWatchLogs$LogGroupName`).
pub fn channel_log_group_name(channel_name: &str, channel_id: &str) -> String {
    format!("/aws/kinesis/{channel_name}/{channel_id}")
}

/// The CloudWatch Logs configuration a channel REPORTS for the settings a request states:
/// the single place the documented default names are resolved, for the create that first
/// states them and the update that re-states them alike.
///
/// A channel that logs and names neither takes the documented defaults ([`channel_log_group_name`],
/// [`DEFAULT_CHANNEL_LOG_STREAM_NAME`]); a channel that logs nothing needs no default, so the
/// names are reported exactly as they arrived.
pub fn resolved_channel_logging(
    enabled: bool,
    log_group_name: Option<String>,
    log_stream_name: Option<String>,
    channel_name: &str,
    channel_id: &str,
) -> ChannelLoggingConfiguration {
    if !enabled {
        return ChannelLoggingConfiguration {
            cloud_watch_logs: CloudWatchLogs {
                enabled: false,
                log_group_name,
                log_stream_name,
            },
        };
    }
    ChannelLoggingConfiguration {
        cloud_watch_logs: CloudWatchLogs {
            enabled: true,
            log_group_name: Some(
                log_group_name
                    .unwrap_or_else(|| channel_log_group_name(channel_name, channel_id)),
            ),
            log_stream_name: Some(
                log_stream_name.unwrap_or_else(|| DEFAULT_CHANNEL_LOG_STREAM_NAME.to_string()),
            ),
        },
    }
}

/// Whether `key_id` is shaped like a KMS key identifier at all.
///
/// The forms one may take are the ones this service publishes for every member that names a
/// KMS key: "a globally unique identifier, a fully specified ARN to either an alias or a
/// key, or an alias name prefixed by 'alias/'"
/// (`StartStreamEncryptionInput$KeyId`) — so a key ARN, an alias ARN, a globally unique key
/// id, or an alias name. A value outside all four names no key that could exist, which is a
/// malformed request rather than a key lookup.
pub fn is_kms_key_identifier(key_id: &str) -> bool {
    if let Some(arn_fields) = key_id.strip_prefix(ARN_PREFIX) {
        return is_kms_arn(arn_fields);
    }
    if let Some(alias) = key_id.strip_prefix(KMS_ALIAS_PREFIX) {
        return !alias.is_empty();
    }
    is_kms_key_uuid(key_id)
}

/// Whether the fields after an ARN's `arn:` prefix spell a KMS key or alias ARN: the
/// partition, the `kms` service, a region, this account's twelve digits, and a `key/<id>`
/// or `alias/<name>` resource.
fn is_kms_arn(arn_fields: &str) -> bool {
    let mut fields = arn_fields.split(':');
    let partition = fields.next().unwrap_or_default();
    let service = fields.next().unwrap_or_default();
    let region = fields.next().unwrap_or_default();
    let account = fields.next().unwrap_or_default();
    let resource = fields.next().unwrap_or_default();
    // A KMS ARN has exactly these six fields, so anything past the resource is not one.
    if fields.next().is_some() {
        return false;
    }
    let names_a_key = resource
        .strip_prefix(KMS_KEY_RESOURCE_PREFIX)
        .is_some_and(is_kms_key_uuid);
    let names_an_alias = resource
        .strip_prefix(KMS_ALIAS_PREFIX)
        .is_some_and(|alias| !alias.is_empty());
    partition.starts_with("aws")
        && service == "kms"
        && is_region_name(region)
        && account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
        && (names_a_key || names_an_alias)
}

/// How many digits an account id segment of an ARN carries, which every ARN pattern of this
/// model spells `\d{12}`.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The groups of hexadecimal digits a globally unique key id is written in: "Globally
/// unique key ID example: 12345678-1234-1234-1234-123456789012"
/// (`StartStreamEncryptionInput$KeyId`).
const KMS_KEY_UUID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// Whether `value` is written as a globally unique key id, in the groups
/// [`KMS_KEY_UUID_GROUPS`] names.
fn is_kms_key_uuid(value: &str) -> bool {
    let mut groups = value.split('-');
    KMS_KEY_UUID_GROUPS.iter().all(|width| {
        groups.next().is_some_and(|group| {
            group.len() == *width && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && groups.next().is_none()
}

/// Whether `key_id` names the KMS key this service owns rather than a customer managed one,
/// by its alias or by the alias ARN that resolves to it: "Master key owned by Kinesis Data
/// Streams: alias/aws/kinesis" (`StartStreamEncryptionInput$KeyId`).
pub fn names_the_service_key(key_id: &str) -> bool {
    key_id == KINESIS_SERVICE_KEY_ALIAS
        || (key_id.starts_with(ARN_PREFIX)
            && key_id.split(':').next_back() == Some(KINESIS_SERVICE_KEY_ALIAS))
}

/// The id of the `index`th shard of a stream: `shardId-` and the index in twelve digits,
/// inside the `ShardId` pattern `^[a-zA-Z0-9_.-]+$`.
pub fn shard_id(index: u32) -> String {
    format!("shardId-{index:012}")
}

/// The hash key range each of `shard_count` shards covers, lowest first.
///
/// "Scale-out within a stream is explicitly supported by means of shards, which are
/// uniquely identified groups of data records in a stream" (`CreateStream`): a stream's
/// shards partition the 128-bit hash key space into contiguous, equal, non-overlapping
/// slices, so the ranges run from 0 to 2^128-1 without a gap.
///
/// The map is one entry per shard, so the count is bounded HERE, at the allocation, and not
/// only by the callers: every operation that lays down or re-tiles a shard map refuses a
/// count past the account and region's shard quota
/// ([`ComAmazonawsKinesisKinesis20131202State::shard_quota_in`], which
/// never reports more than [`MAX_SHARDS_PER_REGION`]) with a modelled error before it
/// reaches this function, and the clamp below keeps the allocation bounded whatever asks
/// for it.
pub fn hash_key_ranges(shard_count: u32) -> Vec<(String, String)> {
    // Every caller refuses a larger count with a modelled error, so reaching here with one
    // is a caller bug rather than a request this function may answer: it is caught in a
    // debug build, and the clamp keeps the allocation bounded in a release build.
    debug_assert!(
        shard_count <= MAX_SHARDS_PER_REGION,
        "a shard map is laid down for at most {MAX_SHARDS_PER_REGION} shards"
    );
    let shards = u128::from(shard_count.clamp(1, MAX_SHARDS_PER_REGION));
    // 2^128 is one past `u128::MAX`, so the quotient and remainder of the division by the
    // shard count are taken against `u128::MAX` and the remainder carried back by one.
    let quotient = u128::MAX / shards;
    let remainder = u128::MAX % shards + 1;
    // The lowest hash key of the `index`th shard: floor(index * 2^128 / shard_count).
    let boundary = |index: u128| index * quotient + index * remainder / shards;
    (0..shards)
        .map(|index| {
            let start = boundary(index);
            let end = if index + 1 == shards {
                u128::MAX
            } else {
                boundary(index + 1) - 1
            };
            (start.to_string(), end.to_string())
        })
        .collect()
}

/// The first sequence number the `index`th shard of a stream created at `created_at` can
/// hold, inside the `SequenceNumber` pattern `^(0|([1-9]\d{0,128}))$`.
///
/// A sequence number is opaque to a client, so this one is DERIVED from the stream's own
/// creation instant (in milliseconds) and the shard's position: two shards of one stream
/// differ, two streams created at different instants differ, and the value a read reports
/// never changes.
pub fn starting_sequence_number(created_at: SystemTime, index: u32) -> String {
    let millis = epoch_millis(created_at);
    format!("49{millis:013}{index:06}")
}

/// How many digits a record's sequence number appends to the shard's starting sequence
/// number. Fixed width, so every sequence number of one shard is the same length and
/// comparing two of them digit by digit is their numeric order — which is also the order
/// the records were written in.
const RECORD_SEQUENCE_NUMBER_DIGITS: usize = 12;

/// The sequence number of the record a shard writes next: "the unique identifier of the
/// record within its shard" (`Record$SequenceNumber`), inside the `SequenceNumber` pattern
/// `^(0|([1-9]\d{0,128}))$`.
///
/// DERIVED from the shard's own starting sequence number and its write counter, so it lies
/// inside the range the shard publishes (`Shard$SequenceNumberRange`), it is larger than
/// every number written before it, and the value a read reports never changes.
pub fn record_sequence_number(shard_starting_sequence_number: &str, written: u64) -> String {
    format!(
        "{shard_starting_sequence_number}{written:0width$}",
        width = RECORD_SEQUENCE_NUMBER_DIGITS
    )
}

/// Whether `left` is a sequence number no larger than `right`.
///
/// `SequenceNumber` is a decimal integer without leading zeros (`^(0|([1-9]\d{0,128}))$`),
/// which is wider than any integer type this crate holds — so the comparison is by digit
/// COUNT first and then lexicographic, which for that form is the numeric order.
pub fn sequence_number_le(left: &str, right: &str) -> bool {
    (left.len(), left) <= (right.len(), right)
}

/// The `RecordDistributionStrategy` under which the producer's partition key places the
/// record: "If the stream uses the USER_PARTITION_KEY record distribution strategy (the
/// default), a partition key is required" (`PutRecordInput$PartitionKey`).
pub const RECORD_DISTRIBUTION_USER_PARTITION_KEY: &str = "USER_PARTITION_KEY";

/// The `RecordDistributionStrategy` under which the service places the record itself: "If
/// the stream uses the AUTO record distribution strategy, the partition key is optional
/// and any value you provide is ignored, along with any ExplicitHashKey you provide"
/// (`PutRecordInput$PartitionKey`).
pub const RECORD_DISTRIBUTION_AUTO: &str = "AUTO";

/// The 128-bit hash key a partition key maps to: "an MD5 hash function is used to map
/// partition keys to 128-bit integer values and to map associated data records to shards.
/// As a result of this hashing mechanism, all data records with the same partition key map
/// to the same shard within the stream" (`PutRecordInput$PartitionKey`).
///
/// The digest is PROTOCOL-MANDATED record placement, not a security primitive: a client
/// computes the same mapping to predict which shard its records land in, so the published
/// `HashKeyRange` of a shard only means anything with this exact function. Nothing here
/// authenticates, signs or protects anything.
pub fn partition_key_hash(partition_key: &str) -> u128 {
    let digest = <::md5::Md5 as ::md5::Digest>::digest(partition_key.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(digest.as_slice());
    u128::from_be_bytes(bytes)
}

/// The largest hash key a stream's shard map can hold a record at, as the refusal below
/// writes it: "the hash key range of a shard" spans the 128-bit integers an MD5 digest
/// produces (`PutRecordInput$PartitionKey`), so the key space is `[0, 2^128-1]`.
const HASH_KEY_SPACE: &str = "[0, 2^128-1]";

/// How a write refuses a hash-key member whose value lies outside the 128-bit key space
/// altogether: "Invalid `<member>`. `<member>` must be in the range: [0, 2^128-1]. Specified
/// value was `<value>`".
///
/// `HashKey` is modelled `^(0|([1-9]\d{0,38}))$`, which admits 39-digit values past
/// `2^128-1`, so the model cannot catch this one and the service range-checks the value
/// itself. Built FROM the request: the member that carried the value and the value as sent.
/// This is a RANGE violation of the member, which is a different fault from a value inside
/// the key space that no open shard covers ([`hash_key_outside_the_shard_map_message`]).
pub fn hash_key_out_of_range_message(member: &str, value: &str) -> String {
    format!(
        "Invalid {member}. {member} must be in the range: {HASH_KEY_SPACE}. Specified value \
         was {value}"
    )
}

/// How a write states that the request's `ExplicitHashKey` or `PartitionKey` lands in no
/// shard of the stream. Built FROM the request, so the value at fault names it.
pub fn hash_key_outside_the_shard_map_message(hash_key: &str) -> String {
    format!(
        "the hash key {hash_key} lies in no open shard of the stream; an ExplicitHashKey \
         must fall inside one shard's HashKeyRange"
    )
}

/// How a data-plane operation refuses a request that asked to be judged without being
/// served: "Checks if your request will succeed. DryRun is an optional parameter"
/// (`PutRecordInput$DryRun`, and the same sentence on `PutRecords`, `GetShardIterator` and
/// `GetRecords`), so the refusal has to say that the request WOULD have succeeded \u2014 which is
/// what tells it apart from a real refusal \u2014 and which operation judged it.
///
/// Built FROM the call: the only variable part is the name of the operation the request
/// named, so one sentence serves every operation that models the member.
pub fn dry_run_succeeded_message(operation: &str) -> String {
    format!(
        "DryRunOperation validation succeeded while calling {operation} operation.: Request \
         would have succeeded, but DryRun flag is set."
    )
}

/// How a write refuses a request that names no `PartitionKey` for a stream whose records a
/// partition key places: the rule `PutRecordInput$PartitionKey` publishes ("If the stream
/// uses the USER_PARTITION_KEY record distribution strategy (the default), a partition key
/// is required").
pub const PARTITION_KEY_REQUIRED_MESSAGE: &str =
    "PartitionKey is required for a stream that uses the USER_PARTITION_KEY record \
     distribution strategy";

/// The maximum record size a stream is configured for when its create names no
/// `MaxRecordSizeInKiB`, in KiB: "Each data record can be up to 1 MiB in size by default.
/// Amazon Kinesis Data Streams supports large records up to 10 MiB in size" (`GetRecords`)
/// — which is also the smallest value `MaxRecordSizeInKiB` is modelled to take (`range
/// 1024..10240`). Every stream carries a maximum, so this is what a create that names none
/// stores and what a read of such a stream reports.
pub const DEFAULT_MAX_RECORD_SIZE_KI_B: i32 = 1_024;

/// The most bytes one `PutRecords` request carries across its entries: "Each PutRecords
/// request can support up to 500 records. Each record in the request can be as large as 1
/// MiB, up to a limit of 5 MiB for the entire request" (`PutRecords`) — the 500 is the
/// modelled `@length(max: 500)` the boundary validator enforces, this is the aggregate it
/// does not.
pub const MAX_PUT_RECORDS_REQUEST_BYTES: usize = 5 * 1024 * 1024;

/// How a write refuses a batch whose records total more than one request may carry
/// ([`MAX_PUT_RECORDS_REQUEST_BYTES`]). Built FROM the request, so the total at fault and
/// the total allowed both name it.
pub fn batch_too_large_message(size: usize, maximum: usize) -> String {
    format!(
        "the records of this request total {size} bytes, past the {maximum} bytes one \
         PutRecords request may carry"
    )
}

/// How a write refuses the records whose `Data` is longer than the maximum record size of
/// the stream it addresses: one clause per member path at fault, in AWS's envelope.
///
/// A stream's `MaxRecordSizeInKiB` bounds the LENGTH of the DATA BLOB one write carries —
/// that member alone, the partition key beside it being bounded by its own modelled
/// `@length` of 1..256 — so it is stated as a length violation of that member, where the
/// blob sits in the request ([`put_record_data_path`], [`put_records_data_path`]) and the
/// number of bytes it may carry. A blob's CONTENT is not echoed, which is why a clause states
/// the member and the requirement alone; a request that breaks the bound in several members
/// states one clause each, the way the envelope accumulates.
pub fn data_too_long_message(member_paths: &[String], maximum: usize) -> String {
    let clauses: Vec<String> = member_paths
        .iter()
        .map(|member_path| data_too_long_clause(member_path, maximum))
        .collect();
    validation_envelope(&clauses)
}

/// The clause one such member contributes.
fn data_too_long_clause(member_path: &str, maximum: usize) -> String {
    format!(
        "Value at '{member_path}' failed to satisfy constraint: Member must have length less \
         than or equal to {maximum}"
    )
}

/// AWS's envelope over the clauses of ONE refusal, which is also the envelope the
/// framework's own constraint walk renders: `1 validation error detected: <clause>` for one
/// clause, `N validation errors detected: <clause>; <clause>` for several — so a bound this
/// service applies and one the model carries are stated alike.
fn validation_envelope(clauses: &[String]) -> String {
    match clauses {
        [] => String::new(),
        [clause] => format!("1 validation error detected: {clause}"),
        clauses => format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        ),
    }
}

/// Where `PutRecord`'s own `Data` member sits in such a message.
pub fn put_record_data_path() -> String {
    ::aws_smithy_schema_server::constraints::path::member("Data")
}

/// Where the `Data` member of the `position`th entry of a `PutRecords` batch sits in such a
/// message, counting entries the way the constraint walk counts a list element: from one.
pub fn put_records_data_path(position: usize) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        path::member("Records"),
        path::element(position),
        path::member("Data"),
    ])
}

/// How many bytes of the digest a sealed handle carries beside its payload. Eight is enough
/// that altering a handle's bytes is not read as another handle, and keeps the hex form
/// short.
const TOKEN_DIGEST_BYTES: usize = 8;

/// The opaque wire form of `payload`: its bytes with a digest of them appended, hex encoded.
///
/// The digest is what makes a handle VERIFIABLE. A client passes back the handle it was
/// handed rather than constructing one, so a string whose bytes have been altered —
/// corrupted in transit, truncated, or built by the caller — has to fail to verify instead
/// of decoding as a cursor into some other page. Nothing here authenticates anything; the
/// digest only tells an altered handle apart from one this service minted.
fn seal_token(payload: &str) -> String {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(payload.as_bytes());
    let mut sealed = payload.as_bytes().to_vec();
    sealed.extend_from_slice(&digest[..TOKEN_DIGEST_BYTES]);
    ::hex::encode(sealed)
}

/// The payload `token` seals, or `None` when `token` is not a handle this service minted: it
/// is not the hex of a sealed byte string, the digest it carries does not verify those bytes,
/// or the bytes are not text.
fn unseal_token(token: &str) -> Option<String> {
    let sealed = ::hex::decode(token).ok()?;
    let split = sealed.len().checked_sub(TOKEN_DIGEST_BYTES)?;
    let (payload, carried) = sealed.split_at(split);
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(payload);
    if digest[..TOKEN_DIGEST_BYTES] != *carried {
        return None;
    }
    String::from_utf8(payload.to_vec()).ok()
}

/// Why a `NextToken` a request carries names no page of the listing it was handed to.
///
/// The two are DISJOINT, and the modelled refusals they map to are too: a token that does not
/// decode and verify is not one this service ever issued, which is a parameter that "can't be
/// used" (`InvalidArgumentException`), while a token that decodes and verifies but whose mint
/// instant is past the cursor lifetime is the one `ExpiredNextTokenException` publishes ("The
/// pagination token passed to the operation is expired"). Deciding the first before the
/// second is what keeps a corrupt cursor from being reported as a stale one — the two carry
/// opposite advice to a paginating client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextTokenRefusal {
    /// Not a token this service issued for this listing: it does not decode, its digest does
    /// not verify it, it is tagged for another listing, or it names another region.
    NotIssued,
    /// A token this service issued whose lifetime has elapsed ([`PAGE_TOKEN_TTL`]).
    Expired,
}

/// How a listing refuses a `NextToken` that is not one it issued: the sentence Kinesis
/// answers for that error class, whose text is fixed — the token itself is opaque, so there
/// is no part of it a refusal could name.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "Invalid NextToken.";

/// Which listing of ONE stream's members a cursor belongs to, which is what it is tagged
/// with: a string this service never issued, and one minted for ANOTHER listing, both name
/// no page of the listing being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamListing {
    /// `ListShards`, whose cursor names the shard its next page starts at.
    Shards,
    /// `ListStreamConsumers`, whose cursor names the consumer its next page starts at.
    Consumers,
}

impl StreamListing {
    /// The tag this listing's cursors carry.
    const fn tag(self) -> &'static str {
        match self {
            Self::Shards => "listShards",
            Self::Consumers => "listStreamConsumers",
        }
    }
}

/// The wire form of a cursor into one stream's members: the listing it was minted for, the
/// instant it was minted, the instant the stream was created, the member the next page
/// starts at, and the stream's ARN.
///
/// Opaque (hex of a sealed byte string, [`seal_token`]) because the model gives `NextToken`
/// no structure: a client passes back what it was handed rather than constructing one. Two
/// hex characters per byte keeps it far inside the modelled length 1..1048576.
fn encode_stream_page(
    listing: StreamListing,
    issued_at_millis: u128,
    stream_created_at_millis: u128,
    next_name: &str,
    stream_arn: &str,
) -> String {
    seal_token(&format!(
        "{}:{issued_at_millis}:{stream_created_at_millis}:{next_name}:{stream_arn}",
        listing.tag()
    ))
}

/// What `token` carries for `listing` — the instant it was minted, the instant the stream
/// was created, the member the next page starts at and the stream's ARN — or `None` when it
/// is not one this service issued for that listing.
fn decode_stream_page(
    listing: StreamListing,
    token: &str,
) -> Option<(u128, u128, String, String)> {
    let decoded = unseal_token(token)?;
    // The ARN carries colons of its own, so it is the LAST field: the tag, the two
    // instants and the member's name (whose `@pattern` admits no colon) are taken first.
    let mut fields = decoded.splitn(5, ':');
    if fields.next()? != listing.tag() {
        return None;
    }
    let issued_at_millis = fields.next()?.parse().ok()?;
    let stream_created_at_millis = fields.next()?.parse().ok()?;
    let next_name = fields.next()?.to_string();
    let stream_arn = fields.next()?.to_string();
    if next_name.is_empty() || stream_arn.is_empty() {
        return None;
    }
    Some((
        issued_at_millis,
        stream_created_at_millis,
        next_name,
        stream_arn,
    ))
}

/// Milliseconds since the epoch at `at`, which is how an instant travels inside a
/// pagination token.
pub fn epoch_millis(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or(0)
}

/// Where a truncated `ListShards` page continues, which stream's shards it continues, and
/// when the page that handed the token out was served.
///
/// The token "unambiguously identifies the stream" (`ListShardsInput$NextToken`), so it
/// carries the stream's ARN AND the instant that stream was created — a name a delete
/// released and a later create took again is another stream, and the pair names only one of
/// them — beside the shard the next page starts at; and it expires, so it carries the
/// instant it was minted. All of it travels INSIDE the token: a read mints one without
/// writing anything a later caller could see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardPageToken {
    /// The ARN of the stream whose shards are being listed.
    pub stream_arn: String,
    /// When that stream was created, in milliseconds since the epoch.
    pub stream_created_at_millis: u128,
    /// The id of the first shard the page did NOT return, which the next page starts at.
    pub next_shard_id: String,
    /// When the page that handed this token out was served, in milliseconds since the
    /// epoch.
    pub issued_at_millis: u128,
}

impl ShardPageToken {
    /// The token that resumes at `next_shard_id` for the stream `stream_arn` created at
    /// `stream_created_at`, minted at `issued_at`.
    pub fn new(
        stream_arn: &str,
        stream_created_at: SystemTime,
        next_shard_id: &str,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            stream_arn: stream_arn.to_string(),
            stream_created_at_millis: epoch_millis(stream_created_at),
            next_shard_id: next_shard_id.to_string(),
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This token as the opaque `NextToken` a client is handed
    /// ([`encode_stream_page`]).
    pub fn encode(&self) -> String {
        encode_stream_page(
            StreamListing::Shards,
            self.issued_at_millis,
            self.stream_created_at_millis,
            &self.next_shard_id,
            &self.stream_arn,
        )
    }

    /// The token `token` carries, or `None` when it is not one this service issued.
    pub fn decode(token: &str) -> Option<Self> {
        let (issued_at_millis, stream_created_at_millis, next_shard_id, stream_arn) =
            decode_stream_page(StreamListing::Shards, token)?;
        Some(Self {
            stream_arn,
            stream_created_at_millis,
            next_shard_id,
            issued_at_millis,
        })
    }

    /// Whether the token names `stream`: the same ARN, created at the same instant.
    pub fn names(&self, stream: &Stream) -> bool {
        self.stream_arn == stream.arn
            && self.stream_created_at_millis == epoch_millis(stream.created_at)
    }

    /// Whether the token is past its lifetime at `now` ([`LIST_SHARDS_TOKEN_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no page either, so it counts as past
    /// its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        is_past_token_lifetime(self.issued_at_millis, now)
    }

    /// Where `token` resumes this listing at `now`, or why it names no page.
    ///
    /// Decode and verification come FIRST, so the two refusals stay disjoint: see
    /// [`NextTokenRefusal`].
    pub fn resume(token: &str, now: SystemTime) -> Result<Self, NextTokenRefusal> {
        let decoded = Self::decode(token).ok_or(NextTokenRefusal::NotIssued)?;
        if decoded.is_expired(now) {
            return Err(NextTokenRefusal::Expired);
        }
        Ok(decoded)
    }
}

/// Where a truncated `ListStreamConsumers` page continues, which stream's consumers it
/// continues, and when the page that handed the token out was served.
///
/// The token "unambiguously identifies the stream" (`ListStreamConsumersInput$NextToken`),
/// so it carries the stream's ARN AND the instant that stream was created — a name a delete
/// released and a later create took again is another stream, and the pair names only one of
/// them — beside the consumer the next page starts at; and it expires, so it carries the
/// instant it was minted. All of it travels INSIDE the token: a read mints one without
/// writing anything a later caller could see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerPageToken {
    /// The ARN of the stream whose consumers are being listed.
    pub stream_arn: String,
    /// When that stream was created, in milliseconds since the epoch.
    pub stream_created_at_millis: u128,
    /// The name of the first consumer the page did NOT return, which the next page starts
    /// at.
    pub next_consumer_name: String,
    /// When the page that handed this token out was served, in milliseconds since the
    /// epoch.
    pub issued_at_millis: u128,
}

impl ConsumerPageToken {
    /// The token that resumes at `next_consumer_name` for the stream `stream_arn` created
    /// at `stream_created_at`, minted at `issued_at`.
    pub fn new(
        stream_arn: &str,
        stream_created_at: SystemTime,
        next_consumer_name: &str,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            stream_arn: stream_arn.to_string(),
            stream_created_at_millis: epoch_millis(stream_created_at),
            next_consumer_name: next_consumer_name.to_string(),
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This token as the opaque `NextToken` a client is handed
    /// ([`encode_stream_page`]).
    pub fn encode(&self) -> String {
        encode_stream_page(
            StreamListing::Consumers,
            self.issued_at_millis,
            self.stream_created_at_millis,
            &self.next_consumer_name,
            &self.stream_arn,
        )
    }

    /// The token `token` carries, or `None` when it is not one this service issued for this
    /// listing.
    pub fn decode(token: &str) -> Option<Self> {
        let (issued_at_millis, stream_created_at_millis, next_consumer_name, stream_arn) =
            decode_stream_page(StreamListing::Consumers, token)?;
        Some(Self {
            stream_arn,
            stream_created_at_millis,
            next_consumer_name,
            issued_at_millis,
        })
    }

    /// Whether the token names `stream`: the same ARN, created at the same instant.
    pub fn names(&self, stream: &Stream) -> bool {
        self.stream_arn == stream.arn
            && self.stream_created_at_millis == epoch_millis(stream.created_at)
    }

    /// Whether the token is past its lifetime at `now` ([`PAGE_TOKEN_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no page either, so it counts as past
    /// its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        is_past_token_lifetime(self.issued_at_millis, now)
    }

    /// Where `token` resumes this listing at `now`, or why it names no page.
    ///
    /// Decode and verification come FIRST, so the two refusals stay disjoint: see
    /// [`NextTokenRefusal`].
    pub fn resume(token: &str, now: SystemTime) -> Result<Self, NextTokenRefusal> {
        let decoded = Self::decode(token).ok_or(NextTokenRefusal::NotIssued)?;
        if decoded.is_expired(now) {
            return Err(NextTokenRefusal::Expired);
        }
        Ok(decoded)
    }
}

/// Which listing a regional cursor belongs to, which is what it is tagged with: a string
/// this service never issued, and one minted for ANOTHER listing, both name no page of the
/// listing being asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalListing {
    /// `ListStreams`, whose cursor names the stream its next page starts at.
    Streams,
    /// `ListChannels`, whose cursor names the channel its next page starts at.
    Channels,
}

impl RegionalListing {
    /// The tag this listing's cursors carry.
    const fn tag(self) -> &'static str {
        match self {
            Self::Streams => "listStreams",
            Self::Channels => "listChannels",
        }
    }
}

/// Where a truncated page of a regional listing continues, and whose resources it continues.
///
/// Both `ListStreams` and `ListChannels` page a set of resources that a name unique inside
/// one region addresses, so ONE cursor serves them: the listing it was minted for, the
/// region the page was served for, the resource the next page starts at, and the instant it
/// was minted, since it expires. All of it travels INSIDE the token: a read mints one
/// without writing anything a later caller could see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionalPageToken {
    /// The listing this cursor was minted for.
    pub listing: RegionalListing,
    /// The region whose resources are being listed.
    pub region: String,
    /// The name of the first resource the page did NOT return, which the next page starts
    /// at.
    pub next_name: String,
    /// When the page that handed this token out was served, in milliseconds since the
    /// epoch.
    pub issued_at_millis: u128,
}

impl RegionalPageToken {
    /// The token that resumes `listing` over `region` at `next_name`, minted at `issued_at`.
    pub fn new(
        listing: RegionalListing,
        region: &str,
        next_name: &str,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            listing,
            region: region.to_string(),
            next_name: next_name.to_string(),
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This token as the opaque `NextToken` a client is handed.
    ///
    /// Opaque (hex of a sealed byte string, [`seal_token`]) because the model gives
    /// `NextToken` no structure: a client passes back what it was handed rather than
    /// constructing one. Two hex characters per byte keeps it far inside the modelled length
    /// 1..1048576.
    pub fn encode(&self) -> String {
        seal_token(&format!(
            "{}:{}:{}:{}",
            self.listing.tag(),
            self.issued_at_millis,
            self.region,
            self.next_name
        ))
    }

    /// The token `token` carries for `listing`, or `None` when it is not one this service
    /// issued for that listing.
    pub fn decode(listing: RegionalListing, token: &str) -> Option<Self> {
        let decoded = unseal_token(token)?;
        let mut fields = decoded.splitn(4, ':');
        if fields.next()? != listing.tag() {
            return None;
        }
        let issued_at_millis = fields.next()?.parse().ok()?;
        let region = fields.next()?.to_string();
        let next_name = fields.next()?.to_string();
        if region.is_empty() || next_name.is_empty() {
            return None;
        }
        Some(Self {
            listing,
            region,
            next_name,
            issued_at_millis,
        })
    }

    /// Whether the token is past its lifetime at `now` ([`PAGE_TOKEN_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no page either, so it counts as past
    /// its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        is_past_token_lifetime(self.issued_at_millis, now)
    }

    /// Where `token` resumes `listing` over `region` at `now`, or why it names no page.
    ///
    /// Decode and verification come FIRST, so the two refusals stay disjoint: see
    /// [`NextTokenRefusal`]. A cursor whose region is not the one the request was signed for
    /// resumes another region's listing, so it is not a token THIS listing issued either.
    pub fn resume(
        listing: RegionalListing,
        region: &str,
        token: &str,
        now: SystemTime,
    ) -> Result<Self, NextTokenRefusal> {
        let decoded = Self::decode(listing, token).ok_or(NextTokenRefusal::NotIssued)?;
        if decoded.region != region {
            return Err(NextTokenRefusal::NotIssued);
        }
        if decoded.is_expired(now) {
            return Err(NextTokenRefusal::Expired);
        }
        Ok(decoded)
    }
}

/// Whether a pagination token minted at `issued_at_millis` is past [`PAGE_TOKEN_TTL`] at
/// `now`, which is the ONE lifetime rule every cursor of this service is judged by.
fn is_past_token_lifetime(issued_at_millis: u128, now: SystemTime) -> bool {
    let millis = u64::try_from(issued_at_millis).unwrap_or(u64::MAX);
    let Some(issued_at) = UNIX_EPOCH.checked_add(Duration::from_millis(millis)) else {
        return true;
    };
    match now.duration_since(issued_at) {
        Ok(age) => age > PAGE_TOKEN_TTL,
        // Minted after the instant it is being used at, which no page of this emulator
        // ever is.
        Err(_) => true,
    }
}

/// What a shard iterator is tagged with, so a string the service never issued is refused
/// rather than read as a position in a shard.
const SHARD_ITERATOR_TAG: &str = "shardIterator";

/// How long a shard iterator stays usable: "A shard iterator expires 5 minutes after it is
/// returned to the requester" (`GetShardIterator`).
pub const SHARD_ITERATOR_TTL: Duration = Duration::from_secs(300);

/// Where in a shard a reader starts, which is what a shard iterator names.
///
/// The five positions are the `ShardIteratorType` values the model declares
/// (`smithy_query(query="shape",shape="ShardIteratorType")`), reduced to what a reader
/// needs to resolve a position: a sequence number, an instant, or one of the two ends of
/// the shard's retained data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardPosition {
    /// `TRIM_HORIZON` — "the last untrimmed record in the shard in the system, which is the
    /// oldest data record in the shard".
    TrimHorizon,
    /// `LATEST` — "just after the most recent record in the shard, so that you always read
    /// the most recent data in the shard", PINNED when the iterator is handed out: a shard
    /// that already held records is pinned to the newest of them
    /// ([`Self::AfterSequenceNumber`]), so this position is only ever carried by an
    /// iterator issued for a shard that held NONE — and every record such a shard holds
    /// when it is read arrived after the iterator was issued.
    Latest,
    /// `AT_SEQUENCE_NUMBER` — "the position denoted by a specific sequence number".
    AtSequenceNumber(String),
    /// `AFTER_SEQUENCE_NUMBER` — "right after the position denoted by a specific sequence
    /// number".
    AfterSequenceNumber(String),
    /// `AT_TIMESTAMP` — "the position denoted by a specific time stamp", in milliseconds
    /// since the epoch, as `GetShardIteratorInput$Timestamp` documents ("A time stamp is the
    /// Unix epoch date with precision in milliseconds").
    AtTimestamp(u128),
}

impl ShardPosition {
    /// How the position travels inside an iterator: the `ShardIteratorType` that names it
    /// and, for the three types that take one, the value the request supplied.
    fn encoded(&self) -> String {
        match self {
            Self::TrimHorizon => SHARD_ITERATOR_TYPE_TRIM_HORIZON.to_string(),
            Self::Latest => SHARD_ITERATOR_TYPE_LATEST.to_string(),
            Self::AtSequenceNumber(sequence_number) => {
                format!("{SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER}={sequence_number}")
            }
            Self::AfterSequenceNumber(sequence_number) => {
                format!("{SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER}={sequence_number}")
            }
            Self::AtTimestamp(millis) => {
                format!("{SHARD_ITERATOR_TYPE_AT_TIMESTAMP}={millis}")
            }
        }
    }

    /// The position `encoded` carries, or `None` for a field no [`Self::encoded`] produces.
    fn decoded(encoded: &str) -> Option<Self> {
        let (iterator_type, value) = match encoded.split_once('=') {
            Some((iterator_type, value)) => (iterator_type, Some(value)),
            None => (encoded, None),
        };
        match (iterator_type, value) {
            (SHARD_ITERATOR_TYPE_TRIM_HORIZON, None) => Some(Self::TrimHorizon),
            (SHARD_ITERATOR_TYPE_LATEST, None) => Some(Self::Latest),
            (SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER, Some(value)) => {
                (!value.is_empty()).then(|| Self::AtSequenceNumber(value.to_string()))
            }
            (SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER, Some(value)) => {
                (!value.is_empty()).then(|| Self::AfterSequenceNumber(value.to_string()))
            }
            (SHARD_ITERATOR_TYPE_AT_TIMESTAMP, Some(value)) => {
                value.parse().ok().map(Self::AtTimestamp)
            }
            _ => None,
        }
    }
}

/// The `ShardIteratorType` values the model declares
/// (`smithy_query(query="shape",shape="ShardIteratorType")`). Each names one
/// [`ShardPosition`], and the three that take a value are the ones whose documentation says
/// which member supplies it.
pub const SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER: &str = "AT_SEQUENCE_NUMBER";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_AFTER_SEQUENCE_NUMBER: &str = "AFTER_SEQUENCE_NUMBER";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_AT_TIMESTAMP: &str = "AT_TIMESTAMP";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_TRIM_HORIZON: &str = "TRIM_HORIZON";
/// See [`SHARD_ITERATOR_TYPE_AT_SEQUENCE_NUMBER`].
pub const SHARD_ITERATOR_TYPE_LATEST: &str = "LATEST";

/// Which shard of which stream a reader is positioned in, where in it, and when the
/// iterator was handed out.
///
/// "A shard iterator specifies the shard position from which to start reading data records
/// sequentially" (`GetShardIterator`), and the position is all a later `GetRecords` needs,
/// so every part of it travels INSIDE the iterator: handing one out writes nothing a later
/// caller could see. The stream's ARN and creation instant are both carried, so a name a
/// delete released and a later create took again is not read with the earlier stream's
/// iterator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardIteratorToken {
    /// The ARN of the stream the shard belongs to.
    pub stream_arn: String,
    /// When that stream was created, in milliseconds since the epoch.
    pub stream_created_at_millis: u128,
    /// The id of the shard being read.
    pub shard_id: String,
    /// Where in the shard the reader starts.
    pub position: ShardPosition,
    /// When the iterator was handed out, in milliseconds since the epoch.
    pub issued_at_millis: u128,
}

impl ShardIteratorToken {
    /// The iterator that starts at `position` in `shard_id` of `stream`, handed out at
    /// `issued_at`.
    pub fn new(
        stream: &Stream,
        shard_id: &str,
        position: ShardPosition,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            stream_arn: stream.arn.clone(),
            stream_created_at_millis: epoch_millis(stream.created_at),
            shard_id: shard_id.to_string(),
            position,
            issued_at_millis: epoch_millis(issued_at),
        }
    }

    /// This iterator as the opaque `ShardIterator` a client is handed.
    ///
    /// Opaque (hex of a tagged byte string) because the model gives `ShardIterator` no
    /// structure: a client passes back what it was handed rather than constructing one. Two
    /// hex characters per byte keeps a stream ARN, a shard id and a position inside the
    /// modelled length 1..512.
    pub fn encode(&self) -> String {
        hex::encode(format!(
            "{SHARD_ITERATOR_TAG}:{}:{}:{}:{}:{}",
            self.issued_at_millis,
            self.stream_created_at_millis,
            self.shard_id,
            self.position.encoded(),
            self.stream_arn
        ))
    }

    /// The iterator `iterator` carries, or `None` when it is not one this service issued.
    pub fn decode(iterator: &str) -> Option<Self> {
        let decoded = String::from_utf8(hex::decode(iterator).ok()?).ok()?;
        // The ARN carries colons of its own, so it is the LAST field: the tag, the two
        // instants, the shard id (whose `@pattern` admits no colon) and the position (whose
        // own fields carry none) are taken first.
        let mut fields = decoded.splitn(6, ':');
        if fields.next()? != SHARD_ITERATOR_TAG {
            return None;
        }
        let issued_at_millis = fields.next()?.parse().ok()?;
        let stream_created_at_millis = fields.next()?.parse().ok()?;
        let shard_id = fields.next()?.to_string();
        let position = ShardPosition::decoded(fields.next()?)?;
        let stream_arn = fields.next()?.to_string();
        if shard_id.is_empty() || stream_arn.is_empty() {
            return None;
        }
        Some(Self {
            stream_arn,
            stream_created_at_millis,
            shard_id,
            position,
            issued_at_millis,
        })
    }

    /// Whether the iterator names `stream`: the same ARN, created at the same instant.
    pub fn names(&self, stream: &Stream) -> bool {
        self.stream_arn == stream.arn
            && self.stream_created_at_millis == epoch_millis(stream.created_at)
    }

    /// Whether the iterator is past its lifetime at `now` ([`SHARD_ITERATOR_TTL`]).
    ///
    /// A mint instant no clock of this emulator could have produced — one that cannot be
    /// represented, or one still in the future — names no position either, so it counts as
    /// past its lifetime rather than reaching the store.
    pub fn is_expired(&self, now: SystemTime) -> bool {
        let millis = u64::try_from(self.issued_at_millis).unwrap_or(u64::MAX);
        let Some(issued_at) = UNIX_EPOCH.checked_add(Duration::from_millis(millis)) else {
            return true;
        };
        match now.duration_since(issued_at) {
            Ok(age) => age > SHARD_ITERATOR_TTL,
            // Handed out after the instant it is being used at, which no iterator of this
            // emulator ever is.
            Err(_) => true,
        }
    }
}

/// How a read states that the stream the request named holds no shard under the `ShardId`
/// it named, worded the way this service words that error class: "Shard `<shardId>` in
/// stream `<streamName>` under account `<account>` does not exist".
///
/// Built FROM the request and the emulator's own account: the shard id at fault, and the
/// stream NAME, which an identifier that is a `StreamARN` carries in its resource segment
/// ([`stream_name_in`]) and one that is a `StreamName` already is — the way
/// [`stream_not_found_message`] names a missing stream.
pub fn shard_not_found_message(shard_id: &str, stream_identifier: &str) -> String {
    format!(
        "Shard {shard_id} in stream {} under account {} does not exist",
        stream_name_in(stream_identifier),
        ::engine::emulator_account_id()
    )
}

/// How a write states that the stream it addresses has no open shard left to place a
/// record in — every shard of the map has been closed by a reshard.
pub const NO_OPEN_SHARD_MESSAGE: &str =
    "the stream has no open shard to write to; a record is written to an open shard of the \
     shard map";

/// How a read refuses a `ShardIterator` this service never issued: it names no position in
/// any shard, so it is the modelled "a specified parameter ... can't be used" refusal
/// rather than an expiry. The sentence Kinesis answers for that error class.
pub const INVALID_SHARD_ITERATOR_MESSAGE: &str = "Invalid ShardIterator.";

/// How a request whose timestamp member names no instant of the Unix epoch is refused: "A
/// time stamp is the Unix epoch date with precision in milliseconds"
/// (`GetShardIteratorInput$Timestamp`, and the same sentence on `ShardFilter$Timestamp`), so
/// a value that names no such instant is refused rather than read as another.
///
/// ONE sentence for the one rule: every member of this service that carries an epoch date is
/// refused identically rather than each wording it.
pub const TIMESTAMP_NOT_AN_EPOCH_DATE_MESSAGE: &str =
    "Timestamp must be a Unix epoch date with precision in milliseconds";

/// How a read refuses a request whose `StreamARN` names a different stream than its
/// `ShardIterator` does: the iterator already names the stream its position is in, so the
/// pair has to name ONE stream — the same rule [`stream_identifier_mismatch_message`]
/// states for a request that names a stream twice.
pub const ITERATOR_STREAM_MISMATCH_MESSAGE: &str =
    "StreamARN names a different stream than the ShardIterator does; the iterator already \
     identifies the stream its position is in";

/// How a listing refuses a request whose `StreamARN` names a different stream than its
/// `NextToken` does: the token already names the stream whose listing it resumes, so the
/// pair has to name ONE stream — the same rule [`stream_identifier_mismatch_message`]
/// states for a request that names a stream twice.
pub const NEXT_TOKEN_STREAM_MISMATCH_MESSAGE: &str =
    "StreamARN names a different stream than the NextToken does; the token already \
     identifies the stream whose listing it resumes";

/// How a read refuses an iterator past its lifetime: the sentence Kinesis answers for
/// `ExpiredIteratorException` ("The provided iterator exceeds the maximum age allowed"),
/// whose variable parts are the instant the iterator was handed out, the instant it was
/// used at, and the lifetime `GetShardIterator` documents ("A shard iterator expires 5
/// minutes after it is returned to the requester"), all taken from the iterator and the
/// clock.
pub fn iterator_expired_message(issued_at_millis: u128, now_millis: u128) -> String {
    format!(
        "Iterator expired. The iterator was created at time {} while right now it is {} \
         which is further in the past than the tolerated delay of {} milliseconds.",
        instant_text(issued_at_millis),
        instant_text(now_millis),
        SHARD_ITERATOR_TTL.as_millis()
    )
}

/// An instant as a refusal spells one out, in the date-time form the wire carries a
/// modelled `Timestamp` in.
fn instant_text(millis: u128) -> String {
    instant_at(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// [`instant_text`] for an instant a REQUEST carried, which the wire admits on either side
/// of the epoch.
fn instant_at(millis: i64) -> String {
    ::aws_smithy_types::DateTime::from_millis(millis)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// How a request that pairs `NextToken` with a member the token already determines is
/// refused: "Don't specify StreamName or StreamCreationTimestamp if you specify NextToken
/// because the latter unambiguously identifies the stream" (`ListShardsInput$NextToken`),
/// which holds for every listing of this service that hands out a cursor. Built FROM the
/// request, so the member at fault names it.
pub fn next_token_conflict_message(member: &str) -> String {
    format!(
        "{member} cannot be specified together with NextToken, which already identifies \
         what is being listed and where the listing resumes"
    )
}

/// How a read refuses a `NextToken` that is past its lifetime: the rule
/// `ExpiredNextTokenException` publishes ("The pagination token passed to the operation is
/// expired") together with the lifetime a cursor of this service carries
/// ([`PAGE_TOKEN_TTL`]). A token this service never issued is NOT this refusal — it is
/// [`INVALID_NEXT_TOKEN_MESSAGE`].
pub fn next_token_expired_message() -> String {
    format!(
        "the pagination token passed to the operation is expired; a NextToken is usable for \
         {} seconds after the page that returned it",
        PAGE_TOKEN_TTL.as_secs()
    )
}
