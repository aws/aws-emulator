// SPDX-License-Identifier: CC0-1.0

//! The EVENT SOURCE MAPPING slice of this service's state: the lifecycle an
//! `EventSourceMappingConfiguration` moves through, the reading of the event source an
//! `EventSourceArn` names, the per-source defaults the model documents, and the record the
//! mapping operations read and write.
//!
//! It lives in its own module rather than in the already long [`crate::state`], which
//! re-exports every item here, so a handler still names them as `crate::state::…` and the
//! module split is invisible to it. The store that HOLDS the records
//! ([`crate::state::ComAmazonawsLambdaAwsgirapiserviceState::event_source_mappings`]) stays
//! there beside the function slice, and so do the two pollers that read a mapping's source
//! ([`crate::state::sqs_poller`] and [`crate::state::event_source_poller`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AmazonManagedKafkaEventSourceConfig, CreateEventSourceMappingOutput, DestinationConfig,
    DocumentDBEventSourceConfig, EventSourceMappingLoggingConfig,
    EventSourceMappingMetricsConfig, FilterCriteria, ProvisionedPollerConfig, ScalingConfig,
    SelfManagedEventSource, SelfManagedKafkaEventSourceConfig, SourceAccessConfiguration,
};

use super::{metrics, ARN_NAMESPACE};

/// `State` values this slice moves an event source mapping through, from the model's
/// `EventSourceMappingConfiguration$State` documentation: "The state of the event source
/// mapping. It can be one of the following: Creating, Enabling, Enabled, Disabling,
/// Disabled, Updating, or Deleting."
pub const EVENT_SOURCE_MAPPING_STATE_CREATING: &str = "Creating";
/// See [`EVENT_SOURCE_MAPPING_STATE_CREATING`].
pub const EVENT_SOURCE_MAPPING_STATE_ENABLED: &str = "Enabled";
/// See [`EVENT_SOURCE_MAPPING_STATE_CREATING`].
pub const EVENT_SOURCE_MAPPING_STATE_DISABLED: &str = "Disabled";
/// See [`EVENT_SOURCE_MAPPING_STATE_CREATING`]. `DeleteEventSourceMapping` is the one
/// call that moves a mapping into it: "When you delete an event source mapping, it
/// enters a Deleting state and might not be completely deleted for several seconds."
pub const EVENT_SOURCE_MAPPING_STATE_DELETING: &str = "Deleting";
/// See [`EVENT_SOURCE_MAPPING_STATE_CREATING`]. `UpdateEventSourceMapping` is the one
/// call that moves a mapping into it, and `ResourceInUseException` names that state as
/// one a mapping is not available in: "you tried to delete an event source mapping
/// currently UPDATING".
pub const EVENT_SOURCE_MAPPING_STATE_UPDATING: &str = "Updating";

/// See [`EVENT_SOURCE_MAPPING_STATE_CREATING`]: the state a mapping is in while it is
/// being RESUMED, which is the only change either of the two transitional states the
/// model enumerates beside `Creating`, `Updating` and `Deleting` can name.
///
/// `UpdateEventSourceMapping` is the call that can "pause invocation and resume later",
/// and `Enabled` is what asks for it — "when true, the event source mapping is active.
/// When false, Lambda pauses polling and invocation" — so an accepted update that turns
/// polling ON is in force only once the mapping reports `Enabled`, and what it reports
/// until then is this, while an accepted update that changes anything ELSE reports
/// [`EVENT_SOURCE_MAPPING_STATE_UPDATING`]
/// (see [`EventSourceMappingRecord::begin_update`]).
pub const EVENT_SOURCE_MAPPING_STATE_ENABLING: &str = "Enabling";

/// See [`EVENT_SOURCE_MAPPING_STATE_ENABLING`]: the same, for the accepted update that
/// turns polling OFF — the PAUSE, which is in force once the mapping reports `Disabled`.
pub const EVENT_SOURCE_MAPPING_STATE_DISABLING: &str = "Disabling";

/// The `State` a mapping `CreateEventSourceMapping` has just accepted is in, for a create
/// that asked for polling as `enabled` says.
///
/// What a create has to STAND UP is the polling: `Enabled` is "when true, the event source
/// mapping is active. When false, Lambda pauses polling and invocation", so a create that
/// asks for an active mapping is ACCEPTED rather than applied — it answers 202 — and the
/// mapping is `Creating` until the poller is up, the move to `Enabled` being owed by the
/// CLOCK ([`EventSourceMappingRecord::materialize`]). A create that asks for a PAUSED
/// mapping asks for no poller at all: there is nothing to stand up and nothing for a
/// client to wait for, so the mapping is in the state that create asked for — the terminal
/// `Disabled` — from the answer onwards, which is what every later read of it reports too.
#[must_use]
pub fn created_event_source_mapping_state(enabled: bool) -> &'static str {
    if enabled {
        EVENT_SOURCE_MAPPING_STATE_CREATING
    } else {
        EVENT_SOURCE_MAPPING_STATE_DISABLED
    }
}

/// `StateTransitionReason`: "Indicates whether a user or Lambda made the last change to
/// the event source mapping." A mapping this store holds only ever changes because a
/// caller asked it to, and Lambda words that differently for the two kinds of poller it
/// stands up: a mapping on a QUEUE reports this, and one on a shard source
/// [`EVENT_SOURCE_MAPPING_USER_ACTION`] (see
/// [`event_source_mapping_transition_reason`]).
pub const EVENT_SOURCE_MAPPING_USER_INITIATED: &str = "USER_INITIATED";

/// See [`EVENT_SOURCE_MAPPING_USER_INITIATED`]: how the same change is worded for a mapping
/// on one of the two SHARD sources — an Amazon Kinesis data stream or an Amazon DynamoDB
/// table's stream — which is how the service worded it when that was observed.
pub const EVENT_SOURCE_MAPPING_USER_ACTION: &str = "User action";

/// What `StateTransitionReason` reports for a change the CALLER asked for on a mapping of
/// the event source `event_source_arn` names.
///
/// One reading for every call that makes such a change — the create, the update and the
/// delete — so no two of them word the same change differently.
#[must_use]
pub fn event_source_mapping_transition_reason(event_source_arn: Option<&str>) -> &'static str {
    if is_shard_source(event_source_arn.and_then(event_source_vendor)) {
        EVENT_SOURCE_MAPPING_USER_ACTION
    } else {
        EVENT_SOURCE_MAPPING_USER_INITIATED
    }
}

/// `LastProcessingResult` for a mapping on a shard source whose poller has not attempted a
/// batch yet: "the result of the event source mapping's last processing attempt" is that no
/// record has been processed, which is a reading of the mapping rather than an unknown.
///
/// It is also what a pass over a queue the broker DOES hold and that holds nothing comes to
/// ([`crate::state::mq_poller`]): the pass read the source and processed no record.
pub const LAST_PROCESSING_RESULT_NO_RECORDS: &str = "No records processed";

/// The `ResourceInUseException` sentence `DeleteEventSourceMapping` refuses a mapping that
/// is not available to be deleted with.
///
/// Entirely FIXED text: it names neither the mapping nor the state it is in, so a caller
/// compares it exactly. `ResourceInUseException` is "The operation conflicts with the
/// resource's availability. For example, you tried to update an event source mapping in
/// the CREATING state, or you tried to delete an event source mapping currently UPDATING"
/// — the delete half of that sentence is the refusal this states.
pub const EVENT_SOURCE_MAPPING_DELETE_IN_USE_MESSAGE: &str =
    "Cannot delete the event source mapping because it is in use.";

/// How long after `CreateEventSourceMapping` or `UpdateEventSourceMapping` the move out
/// of `Creating` or `Updating` becomes due.
///
/// An EMULATOR-CHOSEN span (how long Lambda takes to stand a poller up, or to apply a
/// change to a running one, is not in the model), deliberately non-zero for the same
/// reason as [`crate::state::ACTIVATION_DELAY`]: a caller that reads the mapping straight
/// after creating or updating it sees the `Creating` or `Updating` state the call answered
/// with, and so does a second write arriving inside that window. The instant it becomes
/// due is stored on the record and nothing sleeps for it.
pub const EVENT_SOURCE_MAPPING_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteEventSourceMapping` the mapping's REMOVAL becomes due.
///
/// That operation documents the wait itself — "When you delete an event source mapping,
/// it enters a Deleting state and might not be completely deleted for several seconds" —
/// without stating a duration, so the span is EMULATOR-CHOSEN. It is SEVERAL seconds and
/// not a fraction of one, because the `Deleting` state the delete answers with is a state
/// the model enumerates and a client watches for: every observation of it costs the client
/// another round trip, so a window shorter than one round trip is a transient state no
/// client can ever see, and a client that waits for the mapping to report it waits for
/// something that has already gone. The instant the removal becomes due is stored on the
/// record and nothing sleeps for it (see [`EventSourceMappingRecord::begin_delete`]).
pub const EVENT_SOURCE_MAPPING_DELETE_DELAY: Duration = Duration::from_secs(3);

/// The most event source mappings this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `CreateEventSourceMapping` adds to it, so past this point the slice refuses rather
/// than growing without bound.
pub const MAX_EVENT_SOURCE_MAPPINGS: usize = 1_000;

/// The most event source mappings one `ListEventSourceMappings` page reports, from that
/// operation's own `MaxItems` documentation: "Note that ListEventSourceMappings returns
/// a maximum of 100 items in each response, even if you set the number higher."
pub const MAX_EVENT_SOURCE_MAPPING_PAGE_SIZE: usize = 100;

/// `BatchSize` when the request states none, from the member's own documentation:
/// "Default value: Varies by service. For Amazon SQS, the default is 10. For all other
/// services, the default is 100."
pub const DEFAULT_BATCH_SIZE_QUEUE: i32 = 10;
/// See [`DEFAULT_BATCH_SIZE_QUEUE`].
pub const DEFAULT_BATCH_SIZE_STREAM: i32 = 100;

/// `MaximumBatchingWindowInSeconds` when the request states none, from the member's own
/// documentation: "For Kinesis, DynamoDB, and Amazon SQS event sources, the default
/// batching window is 0 seconds. For Amazon MSK, Self-managed Apache Kafka, Amazon MQ,
/// and DocumentDB event sources, the default batching window is 500 ms" — which is not
/// expressible in this member's unit, so those sources report none at all.
pub const DEFAULT_BATCHING_WINDOW_SECONDS: i32 = 0;

/// `ParallelizationFactor` when the request states none, from the member's own
/// documentation: "(Kinesis and DynamoDB Streams only) The number of batches to process
/// concurrently from each shard. The default value is 1."
pub const DEFAULT_PARALLELIZATION_FACTOR: i32 = 1;

/// `MaximumRecordAgeInSeconds` and `MaximumRetryAttempts` when the request states
/// neither: both are documented as "The default value is -1, which sets the maximum
/// \[age|number of retries\] to infinite."
pub const INFINITE_RETENTION: i32 = -1;

/// The ARN vendor field of the event sources whose documented defaults differ: a queue
/// batches 10 records, a stream 100 ("Amazon Simple Queue Service – The ARN of the
/// queue").
pub const EVENT_SOURCE_VENDOR_QUEUE: &str = "sqs";
/// The two event sources `StartingPosition` is documented as "Required for": "Amazon
/// Kinesis – The ARN of the data stream or a stream consumer" and "Amazon DynamoDB
/// Streams – The ARN of the stream".
pub const EVENT_SOURCE_VENDOR_KINESIS: &str = "kinesis";
/// See [`EVENT_SOURCE_VENDOR_KINESIS`].
pub const EVENT_SOURCE_VENDOR_DYNAMODB: &str = "dynamodb";
/// The ARN vendor field of an Amazon MSK cluster ("Amazon Managed Streaming for Apache
/// Kafka – The ARN of the cluster").
pub const EVENT_SOURCE_VENDOR_KAFKA: &str = "kafka";
/// The ARN vendor field of an Amazon MQ broker ("Amazon MQ – The ARN of the broker").
pub const EVENT_SOURCE_VENDOR_MQ: &str = "mq";
/// The ARN vendor field of an Amazon DocumentDB cluster ("Amazon DocumentDB – The ARN of
/// the DocumentDB change stream"), whose ARNs are in the `rds` namespace.
pub const EVENT_SOURCE_VENDOR_DOCUMENT_DB: &str = "rds";

/// The `EventSourcePosition` value that reads "from which to start reading" at a stated
/// instant, from the model's enum (`AT_TIMESTAMP | LATEST | TRIM_HORIZON`).
pub const STARTING_POSITION_AT_TIMESTAMP: &str = "AT_TIMESTAMP";
/// The `EventSourcePosition` value that starts at the oldest record the source still
/// retains, from that same enum.
pub const STARTING_POSITION_TRIM_HORIZON: &str = "TRIM_HORIZON";
/// The `EventSourcePosition` value that starts just past the records the source already
/// held, from that same enum.
pub const STARTING_POSITION_LATEST: &str = "LATEST";

/// `LastProcessingResult` ("The result of the event source mapping's last processing
/// attempt") after an attempt that delivered its batch to the function.
pub const LAST_PROCESSING_RESULT_OK: &str = "OK";

/// `LastProcessingResult` after an attempt whose invocation of the function was MADE and
/// did not succeed — whatever refused it — in the `PROBLEM: <reason>` form Lambda words an
/// unsuccessful attempt in. The batch is not consumed, so it is read again: a shard source
/// is not checkpointed past it and a queue source deletes none of its messages.
pub const LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED: &str = "PROBLEM: Function call failed";

/// `LastProcessingResult` after a pass of a mapping on an APACHE KAFKA cluster that could not
/// REACH its event source at all, in that same `PROBLEM: <reason>` form — the sentence Lambda
/// words such a mapping's failure to connect in, naming the mapping's own source configuration
/// as what to check.
///
/// It is a reading of the SOURCE and not of the function: a mapping whose cluster Lambda
/// cannot consume from — one that is not there, or one holding none of the topics the mapping
/// names — has made no attempt at a batch, so this is what its last pass came to, and the
/// mapping keeps polling, which is what lets the reading change by itself once the source is
/// in place.
///
/// Each KIND of event source carries its own wording for that same failure — an Amazon MQ
/// broker's is [`LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED`] — because this text is the
/// whole durable record a client reads a broken source off, so none of them is a sentence to
/// word here for a source it was not observed on.
pub const LAST_PROCESSING_RESULT_CONNECTION_ERROR: &str =
    "PROBLEM: Connection error. Please check your event source connection configuration.";

/// `LastProcessingResult` after a pass of a mapping on an AMAZON MQ broker that could not
/// consume its event source, in that same `PROBLEM: <reason>` form: the sentence Lambda words
/// a broker source's failure to be connected to in, which is FIXED text whole — the queue
/// name, the broker, its host and the instant of the pass are no part of it.
///
/// Every way such a pass fails to consume is that one reading: a broker that answers no client
/// yet, one that is gone, and one holding no queue of the name the mapping consumes are each a
/// failure to connect to the source, and none of them is an attempt at a batch. The mapping
/// keeps polling while it reports this — it stays `Enabled` — which is what lets the reading
/// change by itself once the queue is declared.
pub const LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED: &str =
    "PROBLEM: Failed to connect with event source.";

/// Where in ONE shard of a mapping's event source that mapping reads next.
///
/// `StartingPosition` is "The position in a stream from which to start reading" — a
/// property of the MAPPING, fixed when the mapping is created — so the position a shard is
/// read from is this cursor and never the stream's current tip: a mapping that has
/// consumed batches reads on after the newest record it delivered, and one that has
/// consumed none still reads from the position its creation fixed, however long it was
/// paused in between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardCursor {
    /// From the oldest record the shard still retains, which is what `TRIM_HORIZON`
    /// names.
    TrimHorizon,
    /// From a stated instant: the `StartingPositionTimestamp` of an `AT_TIMESTAMP`
    /// mapping, and the mapping's own creation instant for a `LATEST` one — "just after
    /// the most recent record in the shard" AT THE MOMENT the mapping fixed its position
    /// is the same set of records as "everything that arrived from that instant on".
    AtTimestamp(SystemTime),
    /// Past the record this sequence number names: the newest record of the last batch
    /// this mapping delivered from the shard.
    AfterSequenceNumber(String),
}

/// The ARN of an event source mapping:
/// `arn:aws:lambda:<region>:<account>:event-source-mapping:<uuid>`, as the model's
/// `EventSourceMappingArn` pattern requires.
#[must_use]
pub fn event_source_mapping_arn(region: &str, uuid: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("event-source-mapping:{uuid}"),
    )
}

/// A fresh event source mapping identifier. The model's `UUIDString` is
/// `@length(min: 36, max: 36)` and `EventSourceMappingArn`'s pattern spells out the
/// hyphenated UUID form, so the id is a UUID.
#[must_use]
pub fn mint_event_source_mapping_uuid() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// The instant an event source mapping's `LastModified` reports for `at`.
///
/// `LastModified` is "The date that the event source mapping was last updated or that its
/// state changed", and the model puts no `@timestampFormat` on
/// `com.amazonaws.lambda#Date` (`smithy_query(query="shape",shape="Date")` reports a bare
/// `timestamp`), so this member travels in the form this `restJson1` service's protocol
/// gives a timestamp: the epoch-seconds NUMBER. What this adds is the RESOLUTION the
/// service reports that date at — WHOLE seconds, which is what the recorded answers for a
/// mapping carry — so the member is the date of the change and not a reading of the
/// emulator's clock taken to the microsecond, which is a value no answer of the service
/// has the precision to carry.
///
/// Truncating towards the epoch keeps the ORDER of two instants, so a client that compares
/// the member across two reads, or across two mappings, still never sees it go backwards;
/// two changes inside one second report the same date, exactly as the service's own
/// resolution makes them.
#[must_use]
pub fn reported_date(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(::aws_smithy_types::DateTime::from(at).secs())
}

/// The service an `EventSourceArn` names, which is what the per-source defaults and
/// requirements documented for `BatchSize`, `MaximumBatchingWindowInSeconds` and
/// `StartingPosition` are keyed on.
///
/// Read through `service_common::arn` — the one parse this repository states for a
/// caller-supplied ARN — so `None` means the value is not a resource ARN at all.
#[must_use]
pub fn event_source_vendor(event_source_arn: &str) -> Option<&str> {
    service_common::arn::vendor(event_source_arn)
}

/// Which of the SHARD sources an event source ARN's vendor field names — the kind of
/// source whose records sit in shards, read from a position the mapping carries.
///
/// The two of them are the pair the model names together wherever it scopes a member to
/// them: `ParallelizationFactor` and `TumblingWindowInSeconds` are "(Kinesis and DynamoDB
/// Streams only)" and `StartingPosition` is "Required for Amazon Kinesis and Amazon
/// DynamoDB Stream event sources". This enumeration is the ONE statement of that set:
/// [`is_shard_source`] is derived from it, and it is what the reader of those sources
/// dispatches on ([`crate::state::event_source_poller`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardSource {
    /// An Amazon Kinesis data stream (`arn:…:kinesis:…:stream/<name>`).
    Kinesis,
    /// An Amazon DynamoDB table's stream
    /// (`arn:…:dynamodb:…:table/<name>/stream/<label>`).
    DynamoDbStream,
}

impl ShardSource {
    /// The shard source `vendor` names, or `None` for a vendor whose records do not sit in
    /// shards.
    #[must_use]
    pub fn of_vendor(vendor: &str) -> Option<Self> {
        match vendor {
            EVENT_SOURCE_VENDOR_KINESIS => Some(Self::Kinesis),
            EVENT_SOURCE_VENDOR_DYNAMODB => Some(Self::DynamoDbStream),
            _ => None,
        }
    }
}

/// Whether `vendor` names one of the two sources that deliver records per SHARD, which
/// is the scope the model gives `ParallelizationFactor` and `TumblingWindowInSeconds`
/// ("Kinesis and DynamoDB Streams only") and the two sources `StartingPosition` is
/// "Required for".
#[must_use]
pub fn is_shard_source(vendor: Option<&str>) -> bool {
    vendor.and_then(ShardSource::of_vendor).is_some()
}

/// Whether the event source is one of the STREAM sources the model scopes its error
/// handling options to: "The following error handling options are available for stream
/// sources (DynamoDB, Kinesis, Amazon MSK, and self-managed Apache Kafka)".
///
/// `self_managed` says whether the request named a self-managed Apache Kafka cluster
/// instead of an event source ARN, which is a stream source with no ARN to read a vendor
/// from.
#[must_use]
pub fn is_stream_source(vendor: Option<&str>, self_managed: bool) -> bool {
    self_managed || is_shard_source(vendor) || vendor == Some(EVENT_SOURCE_VENDOR_KAFKA)
}

/// Whether the event source is one of the two KAFKA ones the model names together wherever
/// it scopes a member to them — "(Amazon MSK and self-managed Apache Kafka)": an Amazon MSK
/// cluster or VPC connection named by `EventSourceArn`, or the `SelfManagedEventSource` of a
/// self-managed Apache Kafka cluster.
///
/// `self_managed` says whether the request named that cluster instead of an event source
/// ARN, which is a Kafka source with no ARN to read a vendor from.
#[must_use]
pub fn is_kafka_source(vendor: Option<&str>, self_managed: bool) -> bool {
    self_managed || vendor == Some(EVENT_SOURCE_VENDOR_KAFKA)
}

/// What a request whose `MetricsConfig` cannot be satisfied is refused with, and `None` for
/// a configuration the mapping's event source supports.
///
/// `EventSourceMappingMetricsConfig$Metrics` is "the metrics you want your event source
/// mapping to produce", which is a SET of the values the enum declares drawn from the ones
/// the source the mapping polls supports ([`metrics::supported_metrics`]). A selection can
/// fall short of that in two INDEPENDENT ways, each with its own refusal:
///
/// * it repeats a value, asking for one metric twice — a fault of the list by itself, decided
///   without resolving the event source at all, so it is read first and answers the fixed
///   sentence [`metrics::duplicate_metrics_refusal`] carries, whichever value repeats and
///   whatever the source is;
/// * it names a value the source cannot produce — two of the three are documented for the
///   Kafka event sources alone, so a source with no consumers or processing errors to report
///   cannot produce those at all — which is refused with the values that source DOES expect
///   ([`metrics::unsupported_metrics_refusal`]).
///
/// Each is refused with the modelled 400 the operations wrap this message in, and nothing is
/// stored or changed: a mapping advertising a metric vocabulary its source cannot apply would
/// report a configuration no poller could honour.
#[must_use]
pub fn metrics_config_refusal(
    metrics_config: Option<&crate::ops::types::EventSourceMappingMetricsConfig>,
    vendor: Option<&str>,
    self_managed: bool,
) -> Option<String> {
    let metrics = metrics_config.and_then(|config| config.metrics.as_deref())?;
    metrics::duplicate_metrics_refusal(metrics).or_else(|| {
        metrics::unsupported_metrics_refusal(metrics, is_kafka_source(vendor, self_managed))
    })
}

/// Whether `scaling_config` configures NO scaling limit at all — the value that REMOVES a
/// mapping's scaling configuration: "(Amazon SQS only) The scaling configuration for the
/// event source. To remove the configuration, pass an empty value" (`ScalingConfig`).
///
/// What a scaling configuration configures is the one member it carries —
/// `ScalingConfig$MaximumConcurrency`, which "limits the number of concurrent instances that
/// the Amazon SQS event source can invoke" — so one carrying no value for it limits nothing.
/// The calls that STATE a scaling configuration read such a value as no configuration rather
/// than storing a structure that limits nothing, and every reader of the mapping then reports
/// the member not at all. The same rule `FilterCriteria` carrying no filter is held to
/// ([`crate::state::criteria_carry_no_filter`]).
#[must_use]
pub fn scaling_config_sets_no_limit(scaling_config: &crate::ops::types::ScalingConfig) -> bool {
    scaling_config.maximum_concurrency.is_none()
}

/// Whether the event source supports reading from a stated instant, from
/// `StartingPosition`'s own documentation: "AT_TIMESTAMP is supported only for Amazon
/// Kinesis streams, Amazon DocumentDB, Amazon MSK, and self-managed Apache Kafka."
#[must_use]
pub fn supports_at_timestamp(vendor: Option<&str>, self_managed: bool) -> bool {
    self_managed
        || matches!(
            vendor,
            Some(
                EVENT_SOURCE_VENDOR_KINESIS
                    | EVENT_SOURCE_VENDOR_KAFKA
                    | EVENT_SOURCE_VENDOR_DOCUMENT_DB
            )
        )
}

/// Whether a mapping on this event source can be read from `position`.
///
/// `StartingPosition` is "the position in a stream from which to start reading", and the one
/// restriction its documentation states is on reading from a stated INSTANT: "AT_TIMESTAMP is
/// supported only for Amazon Kinesis streams, Amazon DocumentDB, Amazon MSK, and self-managed
/// Apache Kafka" ([`supports_at_timestamp`]). A source outside that set paired with that
/// position is a pairing no poller could open, and is refused with
/// [`unsupported_starting_position_message`].
#[must_use]
pub fn supports_starting_position(vendor: Option<&str>, self_managed: bool, position: &str) -> bool {
    position != STARTING_POSITION_AT_TIMESTAMP || supports_at_timestamp(vendor, self_managed)
}

/// What a `CreateEventSourceMapping` asking for a position the event source it names cannot
/// be read from is refused with, inside the modelled `InvalidParameterValueException`.
///
/// The model states no text for it, so the wording is the service's own, as observed: the
/// refusal is keyed on the KIND of resource the ARN names rather than on the position asked
/// for, and names that ARN bare. The single variable part is the `EventSourceArn` the
/// request sent.
#[must_use]
pub fn unsupported_starting_position_message(event_source_arn: Option<&str>) -> String {
    format!(
        "Unsupported starting position for arn type: {}",
        event_source_arn.unwrap_or_default()
    )
}

/// `BatchSize` when the request states none: see [`DEFAULT_BATCH_SIZE_QUEUE`].
#[must_use]
pub fn default_batch_size(vendor: Option<&str>) -> i32 {
    if vendor == Some(EVENT_SOURCE_VENDOR_QUEUE) {
        DEFAULT_BATCH_SIZE_QUEUE
    } else {
        DEFAULT_BATCH_SIZE_STREAM
    }
}

/// `MaximumBatchingWindowInSeconds` when the request states none: 0 seconds for the
/// three sources documented with that default, and none at all for the sources whose
/// documented default — 500 ms — this member's unit cannot carry. See
/// [`DEFAULT_BATCHING_WINDOW_SECONDS`].
#[must_use]
pub fn default_batching_window_seconds(vendor: Option<&str>) -> Option<i32> {
    (is_shard_source(vendor) || vendor == Some(EVENT_SOURCE_VENDOR_QUEUE))
        .then_some(DEFAULT_BATCHING_WINDOW_SECONDS)
}

/// The ARN resource type of an Amazon Kinesis data stream, which is also the type a
/// stream CONSUMER's ARN carries (`stream/<name>/consumer/<name>/<id>`): "Amazon Kinesis
/// – The ARN of the data stream or a stream consumer."
pub const EVENT_SOURCE_RESOURCE_STREAM: &str = "stream";

/// The ARN resource type an Amazon DynamoDB STREAM is written under: the stream of a
/// table, `table/<name>/stream/<label>`.
pub const EVENT_SOURCE_RESOURCE_TABLE: &str = "table";

/// The segment that makes a DynamoDB `table/…` ARN name the table's STREAM rather than
/// the table itself, which is what "Amazon DynamoDB Streams – The ARN of the stream"
/// asks for.
pub const DYNAMODB_STREAM_SEGMENT: &str = "/stream/";

/// The ARN resource type of an Amazon MSK cluster ("the ARN of the cluster") and of an
/// Amazon DocumentDB cluster ("The ARN of the DocumentDB change stream").
pub const EVENT_SOURCE_RESOURCE_CLUSTER: &str = "cluster";

/// The ARN resource type of the Amazon MSK VPC connection the member documents as the
/// cross-account form: "the ARN of the VPC connection (for cross-account event source
/// mappings)".
pub const EVENT_SOURCE_RESOURCE_VPC_CONNECTION: &str = "vpc-connection";

/// The ARN resource type of an Amazon MQ broker ("Amazon MQ – The ARN of the broker").
pub const EVENT_SOURCE_RESOURCE_BROKER: &str = "broker";

/// Whether `arn` names one of the resources `EventSourceArn` says an event source mapping
/// polls, as that member enumerates them: "Amazon Kinesis – The ARN of the data stream or
/// a stream consumer. Amazon DynamoDB Streams – The ARN of the stream. Amazon Simple Queue
/// Service – The ARN of the queue. Amazon Managed Streaming for Apache Kafka – The ARN of
/// the cluster or the ARN of the VPC connection (for cross-account event source mappings).
/// Amazon MQ – The ARN of the broker. Amazon DocumentDB – The ARN of the DocumentDB change
/// stream."
///
/// Each entry of that list is a resource of ONE vendor namespace and ONE resource type, so
/// an ARN naming any other vendor (a bucket, a topic, a function) or another resource type
/// of one of these vendors (a DynamoDB table rather than its stream) names nothing a
/// mapping can poll, however well formed the ARN is.
#[must_use]
pub fn names_event_source(arn: &::service_common::arn::Arn<'_>) -> bool {
    let resource_type = arn.resource_type();
    match arn.vendor {
        EVENT_SOURCE_VENDOR_KINESIS => resource_type == EVENT_SOURCE_RESOURCE_STREAM,
        // "Amazon DynamoDB Streams – The ARN of the stream" is the stream OF a table,
        // addressed by the table it belongs to and the label that identifies it among that
        // table's streams, so a resource field naming either half as anything but one thing
        // names no stream — read through the one place this crate reads that pair
        // ([`super::event_source_stream_resolution`]), which is the reading the service that
        // OWNS those streams judges its own identifiers by.
        EVENT_SOURCE_VENDOR_DYNAMODB => {
            resource_type == EVENT_SOURCE_RESOURCE_TABLE
                && super::event_source_stream_resolution::names_one_table_stream(arn.resource)
        }
        // A queue's ARN carries the queue NAME as its whole resource field, and a queue
        // name admits neither separator, so a resource field carrying one names no queue.
        EVENT_SOURCE_VENDOR_QUEUE => !arn.resource.contains(['/', ':']),
        EVENT_SOURCE_VENDOR_KAFKA => matches!(
            resource_type,
            EVENT_SOURCE_RESOURCE_CLUSTER | EVENT_SOURCE_RESOURCE_VPC_CONNECTION
        ),
        // A broker is addressed by its NAME and the id Amazon MQ minted for it
        // (`broker:<name>:<id>`), so a resource field that stops at the name names no one
        // broker — which is why the refusal of such a value states that whole format.
        EVENT_SOURCE_VENDOR_MQ => {
            resource_type == EVENT_SOURCE_RESOURCE_BROKER && names_one_broker(arn.resource)
        }
        EVENT_SOURCE_VENDOR_DOCUMENT_DB => resource_type == EVENT_SOURCE_RESOURCE_CLUSTER,
        _ => false,
    }
}

/// Whether the resource field `resource` of an Amazon MQ ARN names ONE broker: the three
/// segments `broker:<name>:<id>` the format of a broker's ARN has, each carrying something.
///
/// A value that stops at the broker's name, or carries an empty segment, names no broker: the
/// id beside the name is the one Amazon MQ minted for that broker, and it is part of how a
/// broker is addressed.
fn names_one_broker(resource: &str) -> bool {
    let mut segments = resource.split(':').skip(1);
    let named = segments.next().is_some_and(|name| !name.is_empty());
    let identified = segments.next().is_some_and(|id| !id.is_empty());
    named && identified && segments.next().is_none()
}

/// Whether the event source `arn` names belongs to the account whose resources this
/// emulator holds.
///
/// A mapping polls the caller's own event source: the ONE cross-account form
/// `EventSourceArn` documents is the Amazon MSK VPC connection ("the ARN of the VPC
/// connection (for cross-account event source mappings)"), so an ARN of any other source
/// carrying another account's id names a resource this account cannot map.
#[must_use]
pub fn event_source_is_mappable_by_this_account(arn: &::service_common::arn::Arn<'_>) -> bool {
    arn.account == ::engine::emulator_account_id()
        || (arn.vendor == EVENT_SOURCE_VENDOR_KAFKA
            && arn.resource_type() == EVENT_SOURCE_RESOURCE_VPC_CONNECTION)
}

/// The `VisibilityTimeout` of the Amazon SQS queue `event_source_arn` names, in seconds,
/// read out of that queue's own store through the shared engine.
///
/// The ARN carries the Region the queue lives in, so the queue is resolved THERE: a queue
/// belongs to one Region's namespace, and an ARN of another Region names a different queue.
///
/// `None` when nothing here can answer for the queue — the value names no queue of this
/// account, this engine carries no queue store, or no queue of that name was created — so
/// there is no configured timeout to compare a function's against, and a mapping on it is
/// accepted on its own terms rather than refused against a default nobody chose.
#[must_use]
pub fn queue_visibility_timeout_seconds<Eng: Send + Sync + 'static>(
    engine: &Eng,
    event_source_arn: &str,
) -> Option<i64> {
    use ::service_com_amazonaws_sqs_amazonsqs as sqs;

    let queues = sqs::state::service_state(engine)?;
    let queues = queues.lock();
    let queue = queues.queue_by_arn(event_source_arn)?;
    Some(queue.integer(sqs::state::VISIBILITY_TIMEOUT))
}

/// Lambda's refusal of a mapping between a queue whose visibility timeout is
/// `visibility_timeout_seconds` and a function whose `Timeout` is
/// `function_timeout_seconds` — `None` when the queue's timeout is at least the
/// function's, which is the combination Lambda accepts.
///
/// A queue hands a message to ONE consumer for the length of its visibility timeout and
/// makes it visible again once that lapses, so a queue whose timeout is shorter than the
/// function's own re-delivers a message that is still being worked on. Lambda compares
/// the two when the mapping between the queue and the function is made, which is the one
/// call that can still refuse the combination.
#[must_use]
pub fn queue_visibility_refusal(
    visibility_timeout_seconds: i64,
    function_timeout_seconds: i32,
) -> Option<String> {
    (visibility_timeout_seconds < i64::from(function_timeout_seconds)).then(|| {
        format!(
            "Queue visibility timeout: {visibility_timeout_seconds} seconds is less than \
             Function timeout: {function_timeout_seconds} seconds"
        )
    })
}

/// One event source mapping this emulator holds.
///
/// `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda
/// Event Function", so what it persists is the mapping the request described: which
/// event source, which function, and every polling, filtering, destination and logging
/// setting it stated, beside the lifecycle this emulator owns. Every member
/// `GetEventSourceMapping` and `ListEventSourceMappings` report comes back out of this
/// record (see [`EventSourceMappingRecord::reported`]), so nothing of a request is
/// echoed straight back to its caller.
///
/// A mapping on an Amazon SQS queue also POLLS that queue once it is enabled, which is
/// what [`crate::state::sqs_poller`] does, and a mapping on a shard source — an Amazon
/// Kinesis data stream or an Amazon DynamoDB table's stream — reads that stream's shards,
/// which is what [`crate::state::event_source_poller`] does —
/// so the record carries the result of the last attempt its poller made
/// ([`Self::last_processing_result`]) and, for a shard source, the mapping's own READING
/// position: where it reads next in each shard of its event source.
#[derive(Debug, Clone)]
pub struct EventSourceMappingRecord {
    /// `UUID`: "The identifier of the event source mapping", which is also the key it is
    /// stored under.
    pub uuid: String,
    /// `EventSourceMappingArn`, minted once at creation; see
    /// [`event_source_mapping_arn`].
    pub arn: String,
    /// `FunctionArn`: "The ARN of the Lambda function", qualified when the request named
    /// a version or an alias.
    pub function_arn: String,
    /// The bare name of the function the mapping invokes, which is what a
    /// `ListEventSourceMappings` `FunctionName` filter resolves to.
    pub function_name: String,
    /// The qualifier the request named the function by, when it named one.
    pub function_qualifier: Option<String>,
    /// `EventSourceArn`: "The Amazon Resource Name (ARN) of the event source", absent
    /// for a self-managed Apache Kafka cluster, which is named by
    /// [`Self::self_managed_event_source`] instead.
    pub event_source_arn: Option<String>,
    /// `Enabled`: "When true, the event source mapping is active. When false, Lambda
    /// pauses polling and invocation." Not reported as such — it is the state the
    /// mapping settles into; see [`EventSourceMappingRecord::materialize`].
    pub enabled: bool,
    /// `BatchSize`: "The maximum number of records in each batch that Lambda pulls from
    /// your stream or queue".
    pub batch_size: i32,
    /// `MaximumBatchingWindowInSeconds`, when the source has a default expressible in
    /// seconds or the request stated one.
    pub maximum_batching_window_in_seconds: Option<i32>,
    /// `ParallelizationFactor`, for the two sources it applies to.
    pub parallelization_factor: Option<i32>,
    /// `StartingPosition`: "The position in a stream from which to start reading."
    pub starting_position: Option<String>,
    /// `StartingPositionTimestamp`: "With StartingPosition set to AT_TIMESTAMP, the time
    /// from which to start reading."
    pub starting_position_timestamp: Option<::aws_smithy_types::DateTime>,
    /// `FilterCriteria`: "An object that defines the filter criteria that determine
    /// whether Lambda should process an event."
    pub filter_criteria: Option<FilterCriteria>,
    /// `KMSKeyArn`: the customer managed key "that Lambda uses to encrypt your
    /// function's filter criteria".
    pub kms_key_arn: Option<String>,
    /// `MetricsConfig`: "The metrics configuration for your event source."
    pub metrics_config: Option<EventSourceMappingMetricsConfig>,
    /// `LoggingConfig`: "(Amazon MSK, and self-managed Apache Kafka only) The logging
    /// configuration for your event source."
    pub logging_config: Option<EventSourceMappingLoggingConfig>,
    /// `ScalingConfig`: "(Amazon SQS only) The scaling configuration for the event
    /// source."
    pub scaling_config: Option<ScalingConfig>,
    /// `DestinationConfig`: where "an event after Lambda processes it" is sent.
    pub destination_config: Option<DestinationConfig>,
    /// `MaximumRecordAgeInSeconds`, for the stream sources it applies to.
    pub maximum_record_age_in_seconds: Option<i32>,
    /// `BisectBatchOnFunctionError`, for the stream sources it applies to.
    pub bisect_batch_on_function_error: Option<bool>,
    /// `MaximumRetryAttempts`, for the stream sources it applies to.
    pub maximum_retry_attempts: Option<i32>,
    /// `TumblingWindowInSeconds`: "(Kinesis and DynamoDB Streams only) The duration in
    /// seconds of a processing window".
    pub tumbling_window_in_seconds: Option<i32>,
    /// `Topics`: "The name of the Kafka topic."
    pub topics: Option<Vec<String>>,
    /// `Queues`: "(Amazon MQ) The name of the Amazon MQ broker destination queue to
    /// consume."
    pub queues: Option<Vec<String>>,
    /// `SourceAccessConfigurations`: "An array of the authentication protocol, VPC
    /// components, or virtual host to secure and define your event source."
    pub source_access_configurations: Option<Vec<SourceAccessConfiguration>>,
    /// `SelfManagedEventSource`: "The self-managed Apache Kafka cluster for your event
    /// source."
    pub self_managed_event_source: Option<SelfManagedEventSource>,
    /// `FunctionResponseTypes`: "A list of current response type enums applied to the
    /// event source mapping."
    ///
    /// The response types CURRENTLY applied are always known — none of them is applied
    /// to a mapping whose request named none — so this is a list rather than an optional
    /// one, and [`Self::reported`] renders it whether or not it holds anything.
    pub function_response_types: Vec<String>,
    /// `AmazonManagedKafkaEventSourceConfig`, when the request stated one.
    pub amazon_managed_kafka_event_source_config: Option<AmazonManagedKafkaEventSourceConfig>,
    /// `SelfManagedKafkaEventSourceConfig`, when the request stated one.
    pub self_managed_kafka_event_source_config: Option<SelfManagedKafkaEventSourceConfig>,
    /// `DocumentDBEventSourceConfig`, when the request stated one.
    pub document_db_event_source_config: Option<DocumentDBEventSourceConfig>,
    /// `ProvisionedPollerConfig`: "the provisioned mode configuration for the event
    /// source".
    pub provisioned_poller_config: Option<ProvisionedPollerConfig>,
    /// `Tags`: "A list of tags to apply to the event source mapping", kept in key order
    /// so `ListTags` reports them deterministically.
    pub tags: BTreeMap<String, String>,
    /// The mapping's `State`, one of the values
    /// [`EVENT_SOURCE_MAPPING_STATE_CREATING`] documents.
    pub state: String,
    /// `StateTransitionReason`; see [`EVENT_SOURCE_MAPPING_USER_INITIATED`].
    pub state_transition_reason: String,
    /// When the move out of `Creating` becomes due; see
    /// [`EventSourceMappingRecord::materialize`].
    pub settled_due: SystemTime,
    /// When this mapping's REMOVAL becomes due, once a `DeleteEventSourceMapping` has
    /// asked for it; `None` for a mapping nobody has deleted. See
    /// [`EventSourceMappingRecord::begin_delete`].
    pub removal_due: Option<SystemTime>,
    /// `LastModified`: "The date that the event source mapping was last updated or that
    /// its state changed."
    pub last_modified: SystemTime,
    /// When this mapping was created, which is the instant its reading position is fixed
    /// at; see [`ShardCursor::AtTimestamp`].
    pub created_at: SystemTime,
    /// Where this mapping reads next in each shard it has already read from, keyed by
    /// shard id — and, for a mapping on a Kafka source, in each PARTITION it has already
    /// read from, keyed by the topic and partition that source identifies one by
    /// ([`crate::state::kafka_poller`]); a position absent from the map has not been read
    /// yet and is read from [`Self::initial_shard_cursor`]. Written by the poller
    /// ([`crate::state::event_source_poller`]) as it checkpoints past each delivered
    /// batch.
    pub shard_cursors: BTreeMap<String, ShardCursor>,
    /// `LastProcessingResult`: "The result of the event source mapping's last processing
    /// attempt" — `None` for a mapping whose poller has not had a batch to attempt yet,
    /// which is every mapping at the instant it is created, and for one whose polling is
    /// PAUSED, which has no poller making attempts at all ([`Self::begin_update`]).
    /// Written only by the poller that made the attempt; see
    /// [`crate::state::sqs_poller::poll_due`] and [`crate::state::event_source_poller`].
    pub last_processing_result: Option<String>,
}

impl EventSourceMappingRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transition it applies is the one the clock owed at `now`: a mapping whose
    /// poller has finished being stood up, or whose accepted change has finished being
    /// applied, leaves the transitional state it was in ([`Self::is_settling`]) for the
    /// state its `Enabled` setting asks for — "When true, the event source mapping is
    /// active. When false, Lambda pauses polling and invocation." It would have been owed
    /// at the same instant even if nobody had called, so applying it on a read reports the
    /// truth rather than inventing a change.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.is_settling() && now >= self.settled_due {
            self.state = if self.enabled {
                EVENT_SOURCE_MAPPING_STATE_ENABLED.to_string()
            } else {
                EVENT_SOURCE_MAPPING_STATE_DISABLED.to_string()
            };
            // `LastModified` is "The date that the event source mapping was last updated
            // or that its state changed", and this IS a state change: it changed at the
            // instant the clock owed it, not at the instant somebody happened to read
            // the record, so that is what is stored.
            self.last_modified = self.settled_due;
        }
    }

    /// Whether this mapping is still moving into the state its settings ask for — the
    /// window `Creating`, `Enabling`, `Disabling` and `Updating` name, which the clock
    /// closes at [`Self::settled_due`].
    ///
    /// Read it only after [`Self::materialize`], so a window the clock has already closed
    /// is not reported as still open.
    #[must_use]
    pub fn is_settling(&self) -> bool {
        matches!(
            self.state.as_str(),
            EVENT_SOURCE_MAPPING_STATE_CREATING
                | EVENT_SOURCE_MAPPING_STATE_UPDATING
                | EVENT_SOURCE_MAPPING_STATE_ENABLING
                | EVENT_SOURCE_MAPPING_STATE_DISABLING
        )
    }

    /// Whether this mapping is still being STOOD UP by its own create — the `Creating`
    /// window, which the clock closes at [`Self::settled_due`].
    ///
    /// What it is for: `ResourceInUseException` is "The operation conflicts with the
    /// resource's availability. For example, you tried to update an event source mapping in
    /// the CREATING state, or you tried to delete an event source mapping currently
    /// UPDATING" — so the window a mapping is not available to be UPDATED in is the one its
    /// create opens, while the one it is not available to be DELETED in is the one an
    /// accepted update opens ([`Self::is_updating`]). A mapping that is applying a change it
    /// already acknowledged still takes a further one, and the last change accepted is the
    /// one that stands.
    ///
    /// Read it only after [`Self::materialize`], so a window the clock has already closed
    /// is not read as still open.
    #[must_use]
    pub fn is_creating(&self) -> bool {
        self.state == EVENT_SOURCE_MAPPING_STATE_CREATING
    }

    /// Accept an `UpdateEventSourceMapping` of this mapping, which found it polling (or
    /// paused) as `polled_before` says.
    ///
    /// The operation answers 202 Accepted, so the change is not in effect at the instant
    /// the call returns: the mapping moves into the transitional state of the change it
    /// accepted and the move to the state its `Enabled` setting asks for is owed by the
    /// CLOCK ([`EVENT_SOURCE_MAPPING_SETTLE_DELAY`]) exactly as the move out of `Creating`
    /// is. Nothing sleeps for it.
    ///
    /// WHICH transitional state that is follows from what the update changes, because the
    /// model enumerates one for each: `UpdateEventSourceMapping` is the call that can "pause
    /// invocation and resume later", so an update that turns polling off is the PAUSE and
    /// reports [`EVENT_SOURCE_MAPPING_STATE_DISABLING`] until it is in force, one that turns
    /// it back on is the RESUME and reports [`EVENT_SOURCE_MAPPING_STATE_ENABLING`], and one
    /// that leaves polling exactly as it found it changed the mapping's CONFIGURATION and
    /// reports [`EVENT_SOURCE_MAPPING_STATE_UPDATING`]. So a client that paused a consumer
    /// and is waiting for the pause to take hold reads the pause's own window rather than a
    /// state that names an unrelated change.
    ///
    /// An update that leaves the mapping with polling PAUSED also leaves it with no
    /// processing attempt to report: `LastProcessingResult` is "the result of the event
    /// source mapping's last processing attempt", a reading of the poller that is making
    /// those attempts, and "when false, Lambda pauses polling and invocation" — so the
    /// attempt the stopped poller last made is no longer what the mapping reports, and the
    /// member is absent until a resumed poller attempts a batch of its own. That is what
    /// keeps a mapping nothing is polling from reporting the health of a poll it is no
    /// longer making ([`Self::last_processing_result`]).
    pub fn begin_update(&mut self, now: SystemTime, polled_before: bool) {
        self.state = if self.enabled == polled_before {
            EVENT_SOURCE_MAPPING_STATE_UPDATING
        } else if self.enabled {
            EVENT_SOURCE_MAPPING_STATE_ENABLING
        } else {
            EVENT_SOURCE_MAPPING_STATE_DISABLING
        }
        .to_string();
        if !self.enabled {
            self.last_processing_result = None;
        }
        self.state_transition_reason = self.caller_transition_reason().to_string();
        // "The date that the event source mapping was last updated or that its state
        // changed": both happened now.
        self.last_modified = now;
        self.settled_due = now + EVENT_SOURCE_MAPPING_SETTLE_DELAY;
    }

    /// Accept a `DeleteEventSourceMapping` of this mapping.
    ///
    /// "When you delete an event source mapping, it enters a Deleting state and might not
    /// be completely deleted for several seconds", so what the call persists is that
    /// state and the instant the removal becomes due
    /// ([`EVENT_SOURCE_MAPPING_DELETE_DELAY`]); the removal itself is owed by the CLOCK
    /// and applied by
    /// [`crate::state::ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`].
    /// Nothing sleeps for it.
    pub fn begin_delete(&mut self, now: SystemTime) {
        self.state = EVENT_SOURCE_MAPPING_STATE_DELETING.to_string();
        self.state_transition_reason = self.caller_transition_reason().to_string();
        // "The date that the event source mapping was last updated or that its state
        // changed": the move into `Deleting` is a state change, and it happened now.
        self.last_modified = now;
        self.removal_due = Some(now + EVENT_SOURCE_MAPPING_DELETE_DELAY);
    }

    /// Whether this mapping is being deleted, which is what makes a second delete of it a
    /// conflict with "the resource's availability" rather than another accepted delete.
    #[must_use]
    pub fn is_deleting(&self) -> bool {
        self.removal_due.is_some()
    }

    /// How a change this mapping's CALLER asked for is worded, which depends on the event
    /// source it polls ([`event_source_mapping_transition_reason`]).
    fn caller_transition_reason(&self) -> &'static str {
        event_source_mapping_transition_reason(self.event_source_arn.as_deref())
    }

    /// Whether this mapping polls one of the two SHARD sources, whose poller is the one
    /// Lambda reports a processing result and a user's change differently for.
    fn polls_shards(&self) -> bool {
        is_shard_source(self.event_source_arn.as_deref().and_then(event_source_vendor))
    }

    /// Whether this mapping polls one of the four STREAM sources — the ones
    /// `DestinationConfig` is documented for: "(Kinesis, DynamoDB Streams, Amazon MSK, and
    /// self-managed Apache Kafka) A configuration object that specifies the destination of
    /// an event after Lambda processes it."
    fn polls_a_stream(&self) -> bool {
        is_stream_source(
            self.event_source_arn.as_deref().and_then(event_source_vendor),
            self.self_managed_event_source.is_some(),
        )
    }

    /// Whether a change this mapping already accepted is still being applied — the window
    /// [`Self::begin_update`] opens, whichever of its three transitional states names the
    /// change, and the clock closes at [`Self::settled_due`].
    ///
    /// What it is for: `ResourceInUseException` names that window as the one a mapping is NOT
    /// available to be DELETED in ("you tried to delete an event source mapping currently
    /// UPDATING"), so a teardown asked for inside it is refused until the change the mapping
    /// acknowledged has settled — and a mapping that is still pausing or resuming is applying
    /// an accepted change exactly as one that is still taking a configuration change is, so
    /// the window is the same one however the update was worded. A further UPDATE inside it is
    /// accepted — the window an update is refused inside is the create's
    /// ([`Self::is_creating`]).
    /// Read it only after [`Self::materialize`], so a window the clock has already closed
    /// is not read as still open.
    #[must_use]
    pub fn is_updating(&self) -> bool {
        matches!(
            self.state.as_str(),
            EVENT_SOURCE_MAPPING_STATE_UPDATING
                | EVENT_SOURCE_MAPPING_STATE_ENABLING
                | EVENT_SOURCE_MAPPING_STATE_DISABLING
        )
    }

    /// Whether the clock has already made this mapping's removal due; see
    /// [`EventSourceMappingRecord::begin_delete`].
    #[must_use]
    pub fn is_removed_by(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// The `EventSourceMappingConfiguration` the wire reports for this record, built
    /// entirely out of what is stored.
    ///
    /// The model declares ONE `EventSourceMappingConfiguration`, generated here as
    /// [`CreateEventSourceMappingOutput`]; an operation whose output is generated as its
    /// own struct over that shape carries these members across one for one rather than
    /// rebuilding them from state a second time.
    #[must_use]
    pub fn reported(&self) -> CreateEventSourceMappingOutput {
        CreateEventSourceMappingOutput {
            uuid: Some(self.uuid.clone()),
            event_source_mapping_arn: Some(self.arn.clone()),
            function_arn: Some(self.function_arn.clone()),
            event_source_arn: self.event_source_arn.clone(),
            batch_size: Some(self.batch_size),
            maximum_batching_window_in_seconds: self.maximum_batching_window_in_seconds,
            parallelization_factor: self.parallelization_factor,
            starting_position: self.starting_position.clone(),
            starting_position_timestamp: self.starting_position_timestamp,
            filter_criteria: self.filter_criteria.clone(),
            // "An object that contains details about an error related to filter criteria
            // encryption": this store decrypts nothing and no such error has occurred.
            filter_criteria_error: None,
            kms_key_arn: self.kms_key_arn.clone(),
            metrics_config: self.metrics_config.clone(),
            logging_config: self.logging_config.clone(),
            scaling_config: self.scaling_config.clone(),
            last_modified: Some(reported_date(self.last_modified)),
            // "The result of the event source mapping's last processing attempt": what
            // the mapping's poller recorded — and, until it has attempted a batch, that no
            // record has been processed, which is what the shard pollers report for a
            // mapping that has polled nothing yet
            // ([`LAST_PROCESSING_RESULT_NO_RECORDS`]).
            last_processing_result: self.last_processing_result.clone().or_else(|| {
                self.polls_shards()
                    .then(|| LAST_PROCESSING_RESULT_NO_RECORDS.to_string())
            }),
            state: Some(self.state.clone()),
            state_transition_reason: Some(self.state_transition_reason.clone()),
            // "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) A
            // configuration object that specifies the destination of an event after Lambda
            // processes it": a mapping on one of those sources HAS that configuration — with
            // no destination in it until one is stated — and no other source has one at all.
            destination_config: self.destination_config.clone().or_else(|| {
                self.polls_a_stream().then_some(DestinationConfig {
                    on_failure: Some(crate::ops::types::OnFailure { destination: None }),
                    on_success: None,
                })
            }),
            topics: self.topics.clone(),
            queues: self.queues.clone(),
            source_access_configurations: self.source_access_configurations.clone(),
            self_managed_event_source: self.self_managed_event_source.clone(),
            maximum_record_age_in_seconds: self.maximum_record_age_in_seconds,
            bisect_batch_on_function_error: self.bisect_batch_on_function_error,
            maximum_retry_attempts: self.maximum_retry_attempts,
            tumbling_window_in_seconds: self.tumbling_window_in_seconds,
            // "A list of CURRENT response type enums applied to the event source
            // mapping": a mapping to which none is applied has an empty list of them, not
            // an unknown one, so the member is reported either way.
            function_response_types: Some(self.function_response_types.clone()),
            amazon_managed_kafka_event_source_config: self
                .amazon_managed_kafka_event_source_config
                .clone(),
            self_managed_kafka_event_source_config: self
                .self_managed_kafka_event_source_config
                .clone(),
            document_db_event_source_config: self.document_db_event_source_config.clone(),
            provisioned_poller_config: self.provisioned_poller_config.clone(),
        }
    }

    /// What `ListEventSourceMappings` reports for this record.
    ///
    /// The ONE member a listing reports differently is the one the model says so about:
    /// `FilterCriteria` is "null in the response of ListEventSourceMapping API calls"
    /// when the criteria are encrypted — "You can view this field in plaintext in the
    /// response of GetEventSourceMapping and DeleteEventSourceMapping calls if you have
    /// kms:Decrypt permissions for the correct KMS key" — and a mapping's criteria are
    /// encrypted exactly when it names a key to encrypt them with ("By default, Lambda
    /// does not encrypt your filter criteria object. Specify this property to encrypt
    /// data using your own customer managed key.").
    #[must_use]
    pub fn listed(&self) -> CreateEventSourceMappingOutput {
        let mut reported = self.reported();
        if self.kms_key_arn.is_some() {
            reported.filter_criteria = None;
        }
        reported
    }

    /// Where this mapping reads a shard it has not read from yet, which is what its
    /// `StartingPosition` states: the oldest retained record for `TRIM_HORIZON`, the
    /// stated instant for `AT_TIMESTAMP`, and the mapping's own creation instant for
    /// `LATEST` — "When true, the event source mapping is active. When false, Lambda
    /// pauses polling and invocation", and pausing polling does not move the position, so
    /// a mapping that was created disabled reads from where it was created rather than
    /// from where the stream had got to when it was enabled.
    #[must_use]
    pub fn initial_shard_cursor(&self) -> ShardCursor {
        match self.starting_position.as_deref() {
            Some(STARTING_POSITION_TRIM_HORIZON) => ShardCursor::TrimHorizon,
            Some(STARTING_POSITION_AT_TIMESTAMP) => ShardCursor::AtTimestamp(
                self.starting_position_timestamp
                    .and_then(|stated| SystemTime::try_from(stated).ok())
                    .unwrap_or(self.created_at),
            ),
            _ => ShardCursor::AtTimestamp(self.created_at),
        }
    }

    /// Whether this mapping is one of the function `name` names, qualified by
    /// `qualifier` when the caller qualified it.
    ///
    /// An unqualified reference names the FUNCTION, so it matches every mapping of it;
    /// a qualified one names one version or alias of that function, so it matches only
    /// the mappings created against that qualifier.
    #[must_use]
    pub fn is_of_function(&self, name: &str, qualifier: Option<&str>) -> bool {
        self.function_name == name
            && qualifier.is_none_or(|qualifier| {
                self.function_qualifier.as_deref() == Some(qualifier)
            })
    }
}
