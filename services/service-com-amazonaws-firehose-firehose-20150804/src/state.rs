// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-firehose-firehose-20150804 service.
//!
//! One name-keyed register of Firehose streams, plus the service-level counter that
//! mints record ids. Everything an operation answers is computed from here.
//!
//! ## Clock-owed transitions
//!
//! `CreateDeliveryStream`, `DeleteDeliveryStream` and the two encryption calls are
//! asynchronous: the model's own documentation says a created stream starts in `CREATING`
//! and reaches `ACTIVE` later, that a deleted stream goes to `DELETING` and is removed
//! later, and that enabling SSE sets the encryption status to `ENABLING` "and then to
//! ENABLED" (disabling it, `DISABLING` and then `DISABLED`). No span is
//! documented with a value, so this emulator picks one ([`TRANSITION_DELAY`]) and stores
//! the instant the change comes due. No operation sleeps: every handler calls
//! [`ComAmazonawsFirehoseFirehose20150804State::materialise_due`] first, which applies
//! only the changes the clock has ALREADY made due — a reporting operation never moves a
//! status that was not owed at the same instant anyway.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

pub mod cloudwatch_logs_pipeline;
pub mod delivery;
pub mod kinesis_source;
pub mod producer;

use crate::ops::types::{
    AmazonOpenSearchServerlessDestinationConfiguration,
    AmazonopensearchserviceDestinationConfiguration, DeliveryStreamEncryptionConfiguration,
    DeliveryStreamEncryptionConfigurationInput, ElasticsearchDestinationConfiguration,
    ExtendedS3DestinationConfiguration, HttpEndpointDestinationConfiguration,
    IcebergDestinationConfiguration, ProcessingConfiguration, ProcessorParameter,
    RedshiftDestinationConfiguration, S3DestinationConfiguration, SecretsManagerConfiguration,
    SnowflakeDestinationConfiguration, SourceDescription, SplunkDestinationConfiguration,
    VpcConfiguration, KEY_TYPE_VALUES,
};

/// How long after the starting call a clock-owed stream transition
/// (`CREATING` -> `ACTIVE`, `DELETING` -> removed, `ENABLING` -> `ENABLED`,
/// `DISABLING` -> `DISABLED`) comes due.
///
/// AWS documents neither span, so this is a CHOSEN value: a few seconds, which is short
/// enough for a test to wait for it and long enough that a caller still observes the
/// transient status the real service shows. The span has to outlast the work a caller does
/// between the starting call and the read that follows it — a create resolves every role
/// and key it names against IAM and KMS before it answers, so a client making a handful of
/// control-plane calls spends far more than a few hundred milliseconds — or the transient
/// status would be unobservable, which is a status this emulator never shows rather than
/// an implementation detail.
pub const TRANSITION_DELAY: Duration = Duration::from_secs(3);

/// How long an ingested record stays in a stream's buffer WITHOUT being delivered.
///
/// Documented, not chosen: "Data records sent to Firehose are stored for 24 hours from
/// the time they are added to a Firehose stream as it tries to send the records to the
/// destination" (`PutRecord`). A record the stream delivers leaves the buffer when it is
/// delivered ([`delivery`]); this is how long one the destination cannot take is kept
/// while that is retried.
pub const RECORD_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The default per-Region stream quota: "By default, you can create up to 5,000 Firehose
/// streams per Amazon Web Services Region" (`CreateDeliveryStream`).
pub const MAX_STREAMS_PER_REGION: usize = 5_000;

/// The most tags ONE stream carries.
///
/// Documented, not chosen: "Each Firehose stream can have up to 50 tags"
/// (`TagDeliveryStream`) — the same ceiling as the modelled `TagDeliveryStreamInputTagList`
/// and `ListTagsForDeliveryStreamOutputTagList` length and as `Limit`'s range.
pub const MAX_TAGS_PER_STREAM: usize = 50;

/// The most record data ONE stream keeps buffered.
///
/// An emulator resource budget, NOT an AWS limit: this process holds every buffered
/// record in its own heap, so without a cap a client could put records faster than the
/// 24-hour retention reclaims them and exhaust the process. A put that does not fit is
/// refused with the modelled `ServiceUnavailableException` ("The service is unavailable.
/// Back off and retry the operation. If you continue to see the exception, throughput
/// limits for the Firehose stream may have been exceeded"), and in a batch with that
/// per-record `ErrorCode` — the answer the real service gives when a stream cannot take
/// more. Each record is at most the modelled 1,024,000 bytes, so this bounds the count
/// too; `MAX_BUFFERED_RECORDS_PER_STREAM` bounds it for small records.
pub const MAX_BUFFERED_BYTES_PER_STREAM: usize = 16 * 1024 * 1024;

/// The most record data this service keeps buffered across EVERY stream — the same
/// budget as [`MAX_BUFFERED_BYTES_PER_STREAM`], applied so many streams cannot together
/// exhaust the process.
pub const MAX_BUFFERED_BYTES: usize = 64 * 1024 * 1024;

/// The most record data ONE `PutRecordBatch` request carries, over every record in it.
///
/// Documented, not chosen: "Each record in the request can be as large as 1,000 KB (before
/// base64 encoding), up to a limit of 4 MB for the entire request. These limits cannot be
/// changed" (`PutRecordBatch`). The per-record bound of that sentence is the modelled
/// `Data` length (1,024,000 bytes — 1,000 KB counted in 1,024-byte units), so the 4 MB of
/// the same sentence is counted the same way.
pub const MAX_PUT_RECORD_BATCH_BYTES: usize = 4 * 1024 * 1024;

/// The most records ONE stream keeps buffered, so a flood of tiny records cannot grow
/// the buffer (and the expiry scan over it) without bound either.
pub const MAX_BUFFERED_RECORDS_PER_STREAM: usize = 50_000;

/// The window the combined encryption-call limit is counted over.
///
/// Documented, not chosen: "The StartDeliveryStreamEncryption and
/// StopDeliveryStreamEncryption operations have a combined limit of 25 calls per Firehose
/// stream per 24 hours" (`StartDeliveryStreamEncryption`).
pub const ENCRYPTION_CALL_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// How many `StartDeliveryStreamEncryption` / `StopDeliveryStreamEncryption` calls one
/// stream takes inside [`ENCRYPTION_CALL_WINDOW`] — the 25 of that same sentence.
pub const MAX_ENCRYPTION_CALLS_PER_WINDOW: usize = 25;

/// The `KeyType` a request that names none asks for: "The default setting is
/// AWS_OWNED_CMK" (`DeliveryStreamEncryptionConfigurationInput$KeyType`).
pub const DEFAULT_KEY_TYPE: &str = "AWS_OWNED_CMK";

/// The `DeliveryStreamType` whose source is Direct PUT: "Provider applications access the
/// Firehose stream directly." SSE can be enabled only on a stream of this type — "You can
/// enable SSE for a Firehose stream only if it's a Firehose stream that uses DirectPut as
/// its source" (`StartDeliveryStreamEncryption`).
pub const DIRECT_PUT_STREAM_TYPE: &str = "DirectPut";

/// The IAM service principal Firehose assumes a configuration's `RoleARN` as.
///
/// Every `RoleARN` a Firehose stream carries is a role Firehose itself assumes — to write
/// to the destination, to read the source, to reach a VPC, to read a secret — so one
/// principal answers for all of them.
pub const FIREHOSE_SERVICE_PRINCIPAL: &str = "firehose.amazonaws.com";

/// The modelled `ProcessorType` values that make up Firehose's CloudWatch Logs
/// decompression pipeline: `Decompression`, which inflates the gzip payload CloudWatch
/// Logs delivers, and `CloudWatchLogProcessing`, which extracts the log events out of
/// that payload's `DataMessage` envelope. Both are values of `ProcessorType`
/// (`crate::ops::types::PROCESSOR_TYPE_VALUES`) and neither can process anything a
/// CloudWatch Logs producer did not send, which is why a stream whose data processing is
/// enabled with one of them takes data from CloudWatch Logs ONLY: "Only requests from
/// CloudWatch Logs are supported when CloudWatch Logs decompression is enabled"
/// (`InvalidSourceException`).
pub const CLOUDWATCH_LOGS_PROCESSOR_TYPES: &[&str] = &["CloudWatchLogProcessing", "Decompression"];

/// The `ProcessorParameterName` values in the order the enum DECLARES them
/// (`smithy_query(query="shape", shape="ProcessorParameterName")`), which is the order a
/// processor's normalised parameter list is reported in.
///
/// The same set as the generated `crate::ops::types::PROCESSOR_PARAMETER_NAME_VALUES`,
/// which the scaffold sorts alphabetically and so cannot carry the declaration order; the
/// test suite pins the two against each other so neither can drift.
pub const PROCESSOR_PARAMETER_NAME_ORDER: &[&str] = &[
    "LambdaArn",
    "NumberOfRetries",
    "MetadataExtractionQuery",
    "JsonParsingEngine",
    "RoleArn",
    "BufferSizeInMBs",
    "BufferIntervalInSeconds",
    "SubRecordType",
    "Delimiter",
    "CompressionFormat",
    "DataMessageExtraction",
];

/// The `ProcessorType` Firehose runs by INVOKING another service — an AWS Lambda
/// function — so Firehose itself parameterizes that invocation
/// ([`NUMBER_OF_RETRIES_PARAMETER`], [`ROLE_ARN_PARAMETER`]). Every other modelled
/// processor transforms the record in Firehose itself and takes only the parameters the
/// caller gives it.
pub const LAMBDA_PROCESSOR_TYPE: &str = "Lambda";

/// The `ProcessorParameterName` carrying how often Firehose retries the invocation.
pub const NUMBER_OF_RETRIES_PARAMETER: &str = "NumberOfRetries";

/// The `ProcessorParameterName` carrying the role the invocation is made under.
pub const ROLE_ARN_PARAMETER: &str = "RoleArn";

/// `NumberOfRetries` for an invocation the caller gave no retry count: "Currently the
/// following default values are supported: 3 for NumberOfRetries"
/// (`ProcessorParameter$ParameterName`).
pub const DEFAULT_NUMBER_OF_RETRIES: &str = "3";

/// The `DeliveryStreamEncryptionStatus` a started enable is in while it is in flight:
/// "Firehose first sets the encryption status of the stream to ENABLING".
pub const ENCRYPTION_ENABLING: &str = "ENABLING";

/// The `DeliveryStreamEncryptionStatus` the clock owes a started enable: "and then to
/// ENABLED".
pub const ENCRYPTION_ENABLED: &str = "ENABLED";

/// The `DeliveryStreamEncryptionStatus` a started disable is in while it is in flight:
/// "Firehose first sets the encryption status of the stream to DISABLING".
pub const ENCRYPTION_DISABLING: &str = "DISABLING";

/// The `DeliveryStreamEncryptionStatus` the clock owes a started disable: "and then to
/// DISABLED".
pub const ENCRYPTION_DISABLED: &str = "DISABLED";

/// The `DeliveryStreamEncryptionStatus` a disable that failed leaves behind: encryption is
/// still in force, so a further `StopDeliveryStreamEncryption` has something to disable.
/// Modelled (`DeliveryStreamEncryptionStatus`) but unreachable here, because nothing in
/// this emulator fails an encryption change.
pub const ENCRYPTION_DISABLING_FAILED: &str = "DISABLING_FAILED";

/// The `DeliveryStreamEncryptionStatus` values that mean an SSE change is still in
/// flight, so another encryption call on the same stream is refused.
///
/// Named from the two transient statuses above so the set and the pair cannot drift.
pub const IN_FLIGHT_ENCRYPTION_STATUSES: &[&str] = &[ENCRYPTION_ENABLING, ENCRYPTION_DISABLING];

/// The lifecycle states this emulator puts a stream through, as the wire enum
/// `DeliveryStreamStatus` names them.
///
/// `CREATING_FAILED` / `DELETING_FAILED` are modelled but unreachable here: nothing in
/// the emulator makes a create or a delete fail asynchronously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Creating,
    Active,
    Deleting,
}

impl StreamStatus {
    /// The `DeliveryStreamStatus` value this state renders as on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
        }
    }
}

/// A change the clock owes a stream, applied by [`DeliveryStream::materialise_due`] once
/// its due instant has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingTransition {
    /// `CREATING` -> `ACTIVE` (and `ENABLING` -> `ENABLED` for encryption enabled at
    /// create, which settles with the creation it rode along with).
    Activate,
    /// `DELETING` -> gone from the register.
    Remove,
}

/// The destination configuration a `CreateDeliveryStream` supplied, kept in the variant
/// the caller chose so `DescribeDeliveryStream` can render the matching
/// `*DestinationDescription`.
///
/// Boxed per variant: these configurations are large structures and an unboxed enum
/// would be as big as the largest of them.
#[derive(Debug, Clone)]
pub enum DestinationConfigKind {
    S3(Box<S3DestinationConfiguration>),
    ExtendedS3(Box<ExtendedS3DestinationConfiguration>),
    Redshift(Box<RedshiftDestinationConfiguration>),
    Elasticsearch(Box<ElasticsearchDestinationConfiguration>),
    AmazonOpenSearchService(Box<AmazonopensearchserviceDestinationConfiguration>),
    Splunk(Box<SplunkDestinationConfiguration>),
    HttpEndpoint(Box<HttpEndpointDestinationConfiguration>),
    Snowflake(Box<SnowflakeDestinationConfiguration>),
    AmazonOpenSearchServerless(Box<AmazonOpenSearchServerlessDestinationConfiguration>),
    Iceberg(Box<IcebergDestinationConfiguration>),
}

impl DestinationConfigKind {
    /// The data-processing configuration this destination carries, whatever variant it is.
    ///
    /// `S3DestinationConfiguration` is the one destination the model gives no
    /// `ProcessingConfiguration` member, so a plain Amazon S3 destination has none.
    pub fn processing_configuration(&self) -> Option<&ProcessingConfiguration> {
        match self {
            Self::S3(_) => None,
            Self::ExtendedS3(config) => config.processing_configuration.as_ref(),
            Self::Redshift(config) => config.processing_configuration.as_ref(),
            Self::Elasticsearch(config) => config.processing_configuration.as_ref(),
            Self::AmazonOpenSearchService(config) => config.processing_configuration.as_ref(),
            Self::Splunk(config) => config.processing_configuration.as_ref(),
            Self::HttpEndpoint(config) => config.processing_configuration.as_ref(),
            Self::Snowflake(config) => config.processing_configuration.as_ref(),
            Self::AmazonOpenSearchServerless(config) => config.processing_configuration.as_ref(),
            Self::Iceberg(config) => config.processing_configuration.as_ref(),
        }
    }

    /// The same configuration, to write the parameters Firehose fills in itself into —
    /// see [`Self::normalise_processors`].
    fn processing_configuration_mut(&mut self) -> Option<&mut ProcessingConfiguration> {
        match self {
            Self::S3(_) => None,
            Self::ExtendedS3(config) => config.processing_configuration.as_mut(),
            Self::Redshift(config) => config.processing_configuration.as_mut(),
            Self::Elasticsearch(config) => config.processing_configuration.as_mut(),
            Self::AmazonOpenSearchService(config) => config.processing_configuration.as_mut(),
            Self::Splunk(config) => config.processing_configuration.as_mut(),
            Self::HttpEndpoint(config) => config.processing_configuration.as_mut(),
            Self::Snowflake(config) => config.processing_configuration.as_mut(),
            Self::AmazonOpenSearchServerless(config) => config.processing_configuration.as_mut(),
            Self::Iceberg(config) => config.processing_configuration.as_mut(),
        }
    }

    /// The role Firehose runs this destination's delivery under: the destination's own
    /// `RoleARN`, or — for a destination whose shape declares none (Splunk) or leaves it
    /// optional (an HTTP endpoint) — the role of the intermediate Amazon S3 location it
    /// always carries. This is the role a processor Firehose has to invoke something with
    /// runs under when the caller named none.
    pub fn delivery_role_arn(&self) -> Option<&str> {
        let role_arn = match self {
            Self::S3(config) => config.role_arn.as_str(),
            Self::ExtendedS3(config) => config.role_arn.as_str(),
            Self::Redshift(config) => config.role_arn.as_str(),
            Self::Elasticsearch(config) => config.role_arn.as_str(),
            Self::AmazonOpenSearchService(config) => config.role_arn.as_str(),
            Self::Splunk(config) => config.s3_configuration.role_arn.as_str(),
            Self::HttpEndpoint(config) => config
                .role_arn
                .as_deref()
                .unwrap_or(config.s3_configuration.role_arn.as_str()),
            Self::Snowflake(config) => config.role_arn.as_str(),
            Self::AmazonOpenSearchServerless(config) => config.role_arn.as_str(),
            Self::Iceberg(config) => config.role_arn.as_str(),
        };
        (!role_arn.is_empty()).then_some(role_arn)
    }

    /// Materialise what a stored data-processing configuration reports: the processor
    /// list it holds, and the processor parameters Firehose supplies ITSELF, each
    /// parameter once and in the service's canonical order.
    ///
    /// A described processor carries its EFFECTIVE parameters, not the list as it was
    /// submitted: a processor Firehose runs by invoking another service
    /// ([`LAMBDA_PROCESSOR_TYPE`]) also runs under a role and with a retry count, and
    /// Firehose fills both in — the retry count at its documented default
    /// ([`DEFAULT_NUMBER_OF_RETRIES`]) and the role from the destination's own delivery
    /// role ([`Self::delivery_role_arn`]) — when the caller named neither. The result is
    /// ordered by [`PROCESSOR_PARAMETER_NAME_ORDER`], so a client reads the same list
    /// whatever order it submitted. A configuration that names no processor holds the
    /// empty list: the processors of a configuration a caller supplied are exactly the
    /// ones it named, so there is a list to report either way.
    ///
    /// Called on the two paths that WRITE a destination configuration
    /// (`CreateDeliveryStream`, `UpdateDestination`), so what a describe echoes and what
    /// the delivery path acts on are the one normalised configuration.
    pub fn normalise_processors(&mut self) {
        let delivery_role_arn = self.delivery_role_arn().map(str::to_string);
        let Some(processing) = self.processing_configuration_mut() else {
            return;
        };
        for processor in processing.processors.get_or_insert_default() {
            // A parameter name names ONE parameter, so a name given twice keeps the
            // first value given for it rather than appearing twice.
            let mut parameters: Vec<ProcessorParameter> = Vec::new();
            for parameter in processor.parameters.iter().flatten() {
                if !parameters
                    .iter()
                    .any(|kept| kept.parameter_name == parameter.parameter_name)
                {
                    parameters.push(parameter.clone());
                }
            }

            if processor.r#type == LAMBDA_PROCESSOR_TYPE {
                fill_parameter(
                    &mut parameters,
                    NUMBER_OF_RETRIES_PARAMETER,
                    DEFAULT_NUMBER_OF_RETRIES,
                );
                if let Some(role_arn) = delivery_role_arn.as_deref() {
                    fill_parameter(&mut parameters, ROLE_ARN_PARAMETER, role_arn);
                }
            }

            // A name the model does not declare cannot reach here (`ParameterName`
            // targets the enum, so the boundary refused it), but one would sort after
            // every declared name rather than being dropped.
            parameters.sort_by_key(|parameter| {
                PROCESSOR_PARAMETER_NAME_ORDER
                    .iter()
                    .position(|name| *name == parameter.parameter_name)
                    .unwrap_or(PROCESSOR_PARAMETER_NAME_ORDER.len())
            });
            // A processor the caller gave no parameters and Firehose fills none for
            // ("You don't have to put a processor parameter when you select
            // `AppendDelimiterToRecord`" — `Processor`) keeps carrying none.
            if !parameters.is_empty() {
                processor.parameters = Some(parameters);
            }
        }
    }
}

/// Add `name` with `value` to `parameters` unless the caller already named it: what
/// Firehose fills in is what the request left out.
fn fill_parameter(parameters: &mut Vec<ProcessorParameter>, name: &str, value: &str) {
    if parameters
        .iter()
        .any(|parameter| parameter.parameter_name == name)
    {
        return;
    }
    parameters.push(ProcessorParameter {
        parameter_name: name.to_string(),
        parameter_value: value.to_string(),
    });
}

/// The `ProcessorType` that de-aggregates one record into the sub-records it carries.
pub const RECORD_DE_AGGREGATION_PROCESSOR_TYPE: &str = "RecordDeAggregation";

/// The `ProcessorParameterName` naming the Lambda function a `Lambda` processor transforms
/// records with.
pub const LAMBDA_ARN_PARAMETER: &str = "LambdaArn";

/// The `ProcessorParameterName` naming the engine a `MetadataExtraction` program is written
/// for.
pub const JSON_PARSING_ENGINE_PARAMETER: &str = "JsonParsingEngine";

/// The `ProcessorParameterName` naming the form of the sub-records a `RecordDeAggregation`
/// processor splits a record into.
pub const SUB_RECORD_TYPE_PARAMETER: &str = "SubRecordType";

/// The parameters a processor of each `ProcessorType` cannot run without.
///
/// The model expresses `Parameters` as one generic list for every processor type, so WHICH
/// parameters a given type requires is a rule of the service rather than of the shape: a
/// processor Firehose runs by invoking a function has to be told which function
/// ([`LAMBDA_ARN_PARAMETER`]), one that derives partition keys has to be given the program
/// and the engine it is written for, and one that splits a record has to be told the form
/// of its sub-records. The types absent from this table require none: a processor whose
/// parameters are all optional ("You don't have to put a processor parameter when you
/// select `AppendDelimiterToRecord`" — `Processor`) and the two of the CloudWatch Logs
/// pipeline ([`CLOUDWATCH_LOGS_PROCESSOR_TYPES`]), which Firehose parameterizes from the
/// payload it was handed.
pub const MANDATORY_PROCESSOR_PARAMETERS: &[(&str, &[&str])] = &[
    (LAMBDA_PROCESSOR_TYPE, &[LAMBDA_ARN_PARAMETER]),
    (
        delivery::METADATA_EXTRACTION_PROCESSOR,
        &[
            delivery::METADATA_EXTRACTION_QUERY_PARAMETER,
            JSON_PARSING_ENGINE_PARAMETER,
        ],
    ),
    (
        RECORD_DE_AGGREGATION_PROCESSOR_TYPE,
        &[SUB_RECORD_TYPE_PARAMETER],
    ),
];

/// The refusal for the first processor of `processing` that omits a parameter its type
/// cannot run without; `None` when every processor the configuration lists carries the
/// parameters [`MANDATORY_PROCESSOR_PARAMETERS`] names for its type (and for a destination
/// that carries no data-processing configuration at all).
///
/// The question is about the configuration as SUBMITTED, so it is asked of every processor
/// the list holds whatever `Enabled` says: a configuration is stored either way, and a
/// processor that could never run is a bad argument now rather than at the moment data
/// processing is turned on. Shared by the two paths that WRITE a destination configuration
/// (`CreateDeliveryStream`, `UpdateDestination`), which both report it as the modelled
/// `InvalidArgumentException`.
pub fn incomplete_processor(processing: Option<&ProcessingConfiguration>) -> Option<String> {
    for processor in processing?.processors.iter().flatten() {
        let Some((_, mandatory)) = MANDATORY_PROCESSOR_PARAMETERS
            .iter()
            .find(|(processor_type, _)| *processor_type == processor.r#type)
        else {
            continue;
        };
        let absent = mandatory.iter().find(|name| {
            !processor
                .parameters
                .iter()
                .flatten()
                .any(|parameter| parameter.parameter_name == **name)
        });
        if let Some(absent) = absent {
            return Some(crate::service::mandatory_processor_parameter_message(
                absent,
                &processor.r#type,
            ));
        }
    }
    None
}

/// Whether `processing` turns on a processor that only a CloudWatch Logs producer can
/// feed ([`CLOUDWATCH_LOGS_PROCESSOR_TYPES`]).
///
/// A configuration whose `Enabled` is not `true` processes nothing — "Enables or disables
/// data processing" (`ProcessingConfiguration$Enabled`) — so the processors it lists are
/// not in force and gate nothing.
fn enables_cloudwatch_logs_processing(processing: &ProcessingConfiguration) -> bool {
    processing.enabled == Some(true)
        && processing.processors.iter().flatten().any(|processor| {
            CLOUDWATCH_LOGS_PROCESSOR_TYPES.contains(&processor.r#type.as_str())
        })
}

/// One destination of a stream: the id Firehose assigned it and the configuration it was
/// created with.
#[derive(Debug, Clone)]
pub struct Destination {
    /// `DestinationId`, as Firehose formats it: `destinationId-` plus a zero-padded ordinal.
    pub destination_id: String,
    pub config: DestinationConfigKind,
}

/// A record `PutRecord` / `PutRecordBatch` appended to a stream's buffer.
#[derive(Debug, Clone)]
pub struct StoredRecord {
    /// The `RecordId` the put answered with — unique across this emulator's streams.
    pub record_id: String,
    pub data: Vec<u8>,
    pub received_at: SystemTime,
    /// How many delivery attempts this record has already been left by: every attempt its
    /// destination did not take counts one, so the error output of a record that was finally
    /// parked states how many attempts it really cost. Zero for a record no attempt has been
    /// made for yet.
    pub delivery_attempts: u32,
}

/// One Firehose stream.
#[derive(Debug, Clone)]
pub struct DeliveryStream {
    pub name: String,
    pub arn: String,
    pub status: StreamStatus,
    /// The change the clock owes this stream, with the instant it comes due.
    pub pending: Option<(PendingTransition, SystemTime)>,
    /// A `DeliveryStreamType` value.
    pub stream_type: String,
    /// Rendered as the decimal `VersionId` / `CurrentDeliveryStreamVersionId`.
    pub version_id: u64,
    pub create_timestamp: SystemTime,
    pub last_update_timestamp: SystemTime,
    pub source: Option<SourceDescription>,
    pub destinations: Vec<Destination>,
    /// The stream's SSE state, as the last encryption call left it. `None` for a stream
    /// SSE was never configured on, which `DescribeDeliveryStream` reports as the
    /// documented disabled state ([`disabled_encryption`]).
    pub encryption: Option<DeliveryStreamEncryptionConfiguration>,
    /// The `DeliveryStreamEncryptionStatus` the clock owes this stream's encryption
    /// configuration, with the instant it comes due. Enabling and disabling SSE are
    /// asynchronous, so the call that starts one leaves the transient status (`ENABLING`
    /// / `DISABLING`) behind and the clock settles it.
    pub pending_encryption: Option<(String, SystemTime)>,
    /// When each `StartDeliveryStreamEncryption` / `StopDeliveryStreamEncryption` call
    /// landed on this stream, oldest first, so their combined windowed limit is counted
    /// from state. Entries older than [`ENCRYPTION_CALL_WINDOW`] are dropped by
    /// [`Self::materialise_due`].
    pub encryption_calls: Vec<SystemTime>,
    pub tags: BTreeMap<String, String>,
    pub records: Vec<StoredRecord>,
    /// The bytes [`Self::records`] holds, kept as a running total so a put checks the
    /// buffer budget without walking the buffer.
    pub buffered_bytes: usize,
    /// The ordinal the next destination of this stream gets.
    pub next_destination_id: u64,
    /// The instant before which no further delivery of this stream's buffer is attempted,
    /// set when a delivery could not be made at all (the destination bucket is not there,
    /// or the object store refused the write) so the retry does not spin — see
    /// [`delivery::DELIVERY_RETRY_SPAN`]. `None` once a delivery succeeds.
    pub delivery_retry_at: Option<SystemTime>,
    /// How far this stream has consumed the SOURCE its configuration names: the sequence
    /// number of the last record buffered from each shard of it, by shard id.
    ///
    /// Empty for a stream whose data comes from direct puts, and for a source-fed stream
    /// that has consumed nothing yet — which reads its source from the instant retrieval
    /// starts at instead ([`kinesis_source`]).
    pub source_positions: BTreeMap<String, String>,
}

impl DeliveryStream {
    /// The id for this stream's next destination, in Firehose's own format.
    pub fn mint_destination_id(&mut self) -> String {
        let ordinal = self.next_destination_id;
        self.next_destination_id += 1;
        format!("destinationId-{ordinal:012}")
    }

    /// Whether server-side encryption is in force for the data this stream takes right
    /// now, which is what `PutRecord`/`PutRecordBatch` report as `Encrypted`. Only a
    /// settled `ENABLED` encrypts: "You can continue to read and write data to your
    /// Firehose stream while the encryption status is ENABLING, but the data is not
    /// encrypted."
    fn is_encrypted(&self) -> bool {
        self.encryption
            .as_ref()
            .is_some_and(|encryption| encryption.status.as_deref() == Some(ENCRYPTION_ENABLED))
    }

    /// Whether this stream's data comes from a source OTHER than direct puts — a Kinesis
    /// data stream, an Amazon MSK cluster or a database — which is what the stream's
    /// `DeliveryStreamType` says: of the modelled values
    /// (`crate::ops::types::DELIVERY_STREAM_TYPE_VALUES`) only
    /// [`DIRECT_PUT_STREAM_TYPE`] is the one where "provider applications access the
    /// Firehose stream directly"; every other value names the service that feeds the
    /// stream instead.
    ///
    /// Such a stream is fed by that source, so the direct-put ingestion operations are not
    /// available on it and a `PutRecord` / `PutRecordBatch` naming it is refused whatever
    /// it carries.
    pub fn takes_a_configured_source(&self) -> bool {
        self.stream_type != DIRECT_PUT_STREAM_TYPE
    }

    /// Whether this stream takes data from CloudWatch Logs only, because one of its
    /// destinations has data processing enabled with a processor of the CloudWatch Logs
    /// decompression pipeline ([`CLOUDWATCH_LOGS_PROCESSOR_TYPES`]).
    ///
    /// A property of the stream's configuration, not of any one payload: every direct
    /// `PutRecord` / `PutRecordBatch` to such a stream is refused with the modelled
    /// `InvalidSourceException`, whatever it carries.
    pub fn takes_cloudwatch_logs_only(&self) -> bool {
        self.destinations.iter().any(|destination| {
            destination
                .config
                .processing_configuration()
                .is_some_and(enables_cloudwatch_logs_processing)
        })
    }

    /// The `DeliveryStreamEncryptionStatus` this stream's SSE is in, if SSE was ever
    /// configured on it.
    pub fn encryption_status(&self) -> Option<&str> {
        self.encryption
            .as_ref()
            .and_then(|encryption| encryption.status.as_deref())
    }

    /// The `DeliveryStreamEncryptionStatus` a caller SEES for this stream — the one
    /// `DescribeDeliveryStream` reports — so a refusal names the status the caller can
    /// read back. A stream SSE was never configured on reports the disabled status
    /// ([`disabled_encryption`]).
    pub fn reported_encryption_status(&self) -> &str {
        self.encryption_status().unwrap_or(ENCRYPTION_DISABLED)
    }

    /// Whether an SSE change this stream started has yet to settle, in which case another
    /// encryption call on it is refused as `ResourceInUseException`.
    pub fn encryption_change_in_flight(&self) -> bool {
        self.encryption_status()
            .is_some_and(|status| IN_FLIGHT_ENCRYPTION_STATUSES.contains(&status))
    }

    /// Charge one `StartDeliveryStreamEncryption` / `StopDeliveryStreamEncryption` call
    /// against this stream's windowed budget, answering `false` — and charging nothing —
    /// when [`MAX_ENCRYPTION_CALLS_PER_WINDOW`] have already landed inside
    /// [`ENCRYPTION_CALL_WINDOW`].
    pub fn charge_encryption_call(&mut self, now: SystemTime) -> bool {
        if self.encryption_calls.len() >= MAX_ENCRYPTION_CALLS_PER_WINDOW {
            return false;
        }
        self.encryption_calls.push(now);
        true
    }

    /// Start an asynchronous SSE change: `configuration` is what a caller sees now (its
    /// status transient) and `settled` is the status the clock owes it.
    pub fn begin_encryption_change(
        &mut self,
        configuration: DeliveryStreamEncryptionConfiguration,
        settled: &str,
        now: SystemTime,
    ) {
        self.encryption = Some(configuration);
        self.pending_encryption = Some((settled.to_string(), now + TRANSITION_DELAY));
        self.last_update_timestamp = now;
    }

    /// Apply every change the clock has already made due to this stream, and report
    /// whether the stream still exists afterwards (`false` once its removal is due).
    /// Adds the bytes any expired record freed to `freed`.
    ///
    /// Writes nothing that was not already owed at `now`, so calling it from a reporting
    /// operation cannot change what a later caller sees.
    fn materialise_due(&mut self, now: SystemTime, freed: &mut usize) -> bool {
        // Records are appended in arrival order, so the expired ones are a prefix: the
        // scan costs what it drops, not what the buffer holds.
        let expired = self.records.partition_point(|record| {
            matches!(now.duration_since(record.received_at), Ok(age) if age >= RECORD_RETENTION)
        });
        if expired > 0 {
            let dropped: usize = self
                .records
                .drain(..expired)
                .map(|record| record.data.len())
                .sum();
            self.buffered_bytes = self.buffered_bytes.saturating_sub(dropped);
            *freed += dropped;
        }

        // The windowed encryption-call budget frees up only with the clock, so dropping
        // the calls that have aged out of the window is owed at `now` too.
        self.encryption_calls.retain(|at| {
            !matches!(now.duration_since(*at), Ok(age) if age >= ENCRYPTION_CALL_WINDOW)
        });

        // An SSE change that has had its span settles to the status it was owed.
        match self.pending_encryption.as_ref() {
            Some((settled, due)) if now >= *due => {
                let settled = settled.clone();
                if let Some(encryption) = self.encryption.as_mut() {
                    encryption.status = Some(settled);
                }
                self.pending_encryption = None;
            }
            _ => {}
        }

        match self.pending {
            Some((PendingTransition::Activate, due)) if now >= due => {
                self.status = StreamStatus::Active;
                self.pending = None;
                if let Some(encryption) = self.encryption.as_mut() {
                    if encryption.status.as_deref() == Some(ENCRYPTION_ENABLING) {
                        encryption.status = Some(ENCRYPTION_ENABLED.to_string());
                    }
                }
                true
            }
            Some((PendingTransition::Remove, due)) if now >= due => false,
            _ => true,
        }
    }
}

/// The SSE state `DescribeDeliveryStream` reports for a stream SSE was never configured
/// on: the documented disabled status and nothing else — with SSE off there is no CMK, so
/// `KeyType` and `KeyARN` are unset ("If KeyType is Amazon Web Services_OWNED_CMK,
/// DeliveryStreamEncryptionConfiguration doesn't contain a value for KeyARN").
///
/// A stream ALWAYS has an SSE state to report: `DeliveryStreamEncryptionConfiguration` is
/// "the server-side encryption (SSE) status for the delivery stream ... You can get
/// DeliveryStreamEncryptionConfiguration by invoking the DescribeDeliveryStream
/// operation", and [`ENCRYPTION_DISABLED`] is the modelled status for "not encrypted".
pub fn disabled_encryption() -> DeliveryStreamEncryptionConfiguration {
    DeliveryStreamEncryptionConfiguration {
        key_arn: None,
        key_type: None,
        status: Some(ENCRYPTION_DISABLED.to_string()),
        failure_description: None,
    }
}

/// Per-service state for com-amazonaws-firehose-firehose-20150804. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsFirehoseFirehose20150804State {
    /// Every stream, keyed by name. A `BTreeMap` so `ListDeliveryStreams` ordering and
    /// `ExclusiveStartDeliveryStreamName` paging are deterministic and alphabetical.
    pub streams: BTreeMap<String, DeliveryStream>,
    /// Monotonic, service-wide, so every minted `RecordId` is distinct.
    pub record_counter: u64,
    /// The bytes every stream's buffer holds together, kept as a running total so a put
    /// checks the service-wide budget without walking the register.
    pub buffered_bytes: usize,
    /// Whether a delivery pump already runs against this store, so at most one does — see
    /// [`delivery::drive_delivery_clock`].
    pub(crate) delivery_pump_running: bool,
    /// Which claim on the delivery pump is the current one, so a pump releases its OWN
    /// claim and never a later pump's.
    pub(crate) delivery_pump_generation: u64,
    /// The running delivery pump's task handle, kept so the pump it replaces is aborted
    /// rather than merely forgotten.
    pub(crate) delivery_pump: Option<::tokio::task::JoinHandle<()>>,
}

impl ComAmazonawsFirehoseFirehose20150804State {
    /// Apply every clock-owed change across the register: activate streams whose
    /// creation span has elapsed, drop streams whose deletion has, and expire records
    /// past their documented retention. A removed stream takes its buffered data with it.
    pub fn materialise_due(&mut self, now: SystemTime) {
        let mut freed = 0;
        self.streams.retain(|_, stream| {
            let keep = stream.materialise_due(now, &mut freed);
            if !keep {
                freed += stream.buffered_bytes;
            }
            keep
        });
        self.buffered_bytes = self.buffered_bytes.saturating_sub(freed);
    }

    /// Append one record to `name`'s buffer: answers the `RecordId` it was stored under
    /// (or `None` when the buffer budget left no room for it, in which case nothing was
    /// stored) and whether server-side encryption was in force.
    pub fn ingest_record(
        &mut self,
        name: &str,
        data: Vec<u8>,
        now: SystemTime,
        producer: producer::Producer,
    ) -> Result<(Option<String>, bool), StreamRefusal> {
        let (stream, counter, buffered_bytes) = self.stream_accepting_records(name, producer)?;
        let encrypted = stream.is_encrypted();
        let record_id = push_record(stream, counter, buffered_bytes, data, now);
        Ok((record_id, encrypted))
    }

    /// Append a batch to `name`'s buffer: answers one entry per record, in the order the
    /// records were given — the `RecordId` it was stored under, or `None` for a record the
    /// buffer had no room for — and whether server-side encryption was in force.
    pub fn ingest_records(
        &mut self,
        name: &str,
        records: Vec<Vec<u8>>,
        now: SystemTime,
        producer: producer::Producer,
    ) -> Result<(Vec<Option<String>>, bool), StreamRefusal> {
        let (stream, counter, buffered_bytes) = self.stream_accepting_records(name, producer)?;
        let encrypted = stream.is_encrypted();
        let record_ids = records
            .into_iter()
            .map(|data| push_record(stream, counter, buffered_bytes, data, now))
            .collect();
        Ok((record_ids, encrypted))
    }

    /// The stream `name` names, if it exists and is accepting data from `producer`.
    ///
    /// Hands back the record counter and the service-wide buffered-byte total beside it
    /// (disjoint field borrows) so a put resolves the stream ONCE and mints an id only for
    /// a record it goes on to store.
    fn stream_accepting_records(
        &mut self,
        name: &str,
        producer: producer::Producer,
    ) -> Result<(&mut DeliveryStream, &mut u64, &mut usize), StreamRefusal> {
        let counter = &mut self.record_counter;
        let buffered_bytes = &mut self.buffered_bytes;
        let Some(stream) = self.streams.get_mut(name) else {
            return Err(StreamRefusal::NoSuchStream);
        };
        // "Attempts to send data to a delivery stream that is not in the ACTIVE state
        // cause an exception."
        if stream.status != StreamStatus::Active {
            return Err(StreamRefusal::NotActive);
        }
        // A stream whose data comes from a configured source is fed by that source, so
        // the direct-put ingestion operations are not available on it: the refusal names
        // the type that says so.
        if stream.takes_a_configured_source() {
            return Err(StreamRefusal::SourceNotDirectPut(stream.stream_type.clone()));
        }
        // A stream configured for the CloudWatch Logs decompression pipeline takes data
        // from CloudWatch Logs only; a put from any other producer is not that producer.
        if stream.takes_cloudwatch_logs_only() && producer != producer::Producer::CloudWatchLogs {
            return Err(StreamRefusal::SourceNotCloudWatchLogs);
        }
        Ok((stream, counter, buffered_bytes))
    }
}

/// Why a stream could take no data at all — the whole request is refused with a modelled
/// error.
///
/// A buffer budget ([`MAX_BUFFERED_BYTES_PER_STREAM`], [`MAX_BUFFERED_BYTES`],
/// [`MAX_BUFFERED_RECORDS_PER_STREAM`]) that leaves no room is NOT one of these: it is
/// reported per record (a `None` record id), because `PutRecordBatch` answers a record it
/// could not store with that record's own entry rather than failing the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamRefusal {
    /// No stream of that name exists.
    NoSuchStream,
    /// The stream exists but is not `ACTIVE`, so it takes no data: the refusal states
    /// that the stream is not active and not which state it is in
    /// (`crate::service::not_active_message`).
    NotActive,
    /// The stream only takes data from CloudWatch Logs
    /// ([`DeliveryStream::takes_cloudwatch_logs_only`]), and the producer this put came
    /// from ([`producer::producer_of`]) is not it: the modelled `InvalidSourceException`.
    SourceNotCloudWatchLogs,
    /// The stream's data comes from a configured source rather than from direct puts
    /// ([`DeliveryStream::takes_a_configured_source`]), so the direct-put ingestion
    /// operations are not available on it: the modelled `InvalidArgumentException`,
    /// carrying the stream's own `DeliveryStreamType`, which the refusal names.
    SourceNotDirectPut(String),
}

/// Buffer `data` under a freshly minted `RecordId` and answer that id, or `None` when the
/// buffer budget leaves no room for it (in which case nothing is stored and no id is
/// minted).
///
/// The id is an opaque token, unique for the life of this emulator because the
/// service-wide counter is part of it.
fn push_record(
    stream: &mut DeliveryStream,
    counter: &mut u64,
    buffered_bytes: &mut usize,
    data: Vec<u8>,
    now: SystemTime,
) -> Option<String> {
    // Each record is at most the modelled 1,024,000 bytes and each budget is far below
    // `usize::MAX`, so these sums cannot overflow.
    if stream.records.len() >= MAX_BUFFERED_RECORDS_PER_STREAM
        || stream.buffered_bytes + data.len() > MAX_BUFFERED_BYTES_PER_STREAM
        || *buffered_bytes + data.len() > MAX_BUFFERED_BYTES
    {
        return None;
    }

    *counter += 1;
    let nanos = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();
    let record_id = format!("{nanos:032x}{counter:016x}");
    stream.buffered_bytes += data.len();
    *buffered_bytes += data.len();
    stream.records.push(StoredRecord {
        record_id: record_id.clone(),
        data,
        received_at: now,
        // No delivery has been attempted for a record that has just been accepted.
        delivery_attempts: 0,
    });
    Some(record_id)
}

/// Why Firehose cannot use the `DeliveryStreamEncryptionConfigurationInput` a caller
/// supplied, as [`check_encryption_configuration_input`] judges it.
///
/// Each variant names the modelled error the caller reports it with:
/// `CreateDeliveryStream` and `StartDeliveryStreamEncryption` both take this input and
/// both model those two errors, so the rules live here, once, beside the state they are
/// checked before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionConfigRefusal {
    /// The configuration itself does not hold together: `InvalidArgumentException`,
    /// "The specified input parameter has a value that is not valid."
    InvalidArgument(String),
    /// KMS cannot serve the named CMK: `InvalidKMSResourceException`, whose `Code`
    /// carries the KMS exception type Firehose relays.
    InvalidKmsResource { code: String, message: String },
}

/// Check a requested SSE configuration against the rules Firehose documents for it, with
/// no regard for which operation asked.
///
/// `region` is the Region the request was signed for — the only Region whose CMKs a
/// stream created by it can reach.
pub fn check_encryption_configuration_input(
    requested: &DeliveryStreamEncryptionConfigurationInput,
    region: &str,
) -> Result<(), EncryptionConfigRefusal> {
    if !KEY_TYPE_VALUES.contains(&requested.key_type.as_str()) {
        return Err(EncryptionConfigRefusal::InvalidArgument(format!(
            "KeyType '{}' is not valid; valid values are {}",
            requested.key_type,
            KEY_TYPE_VALUES.join(", ")
        )));
    }
    // "If you set KeyType to CUSTOMER_MANAGED_CMK, you must specify the Amazon Resource
    // Name (ARN) of the CMK. If you set KeyType to AWS_OWNED_CMK, Firehose uses a
    // service-account CMK."
    match (requested.key_type.as_str(), requested.key_arn.as_ref()) {
        ("CUSTOMER_MANAGED_CMK", None) => {
            return Err(EncryptionConfigRefusal::InvalidArgument(
                "KeyARN has to be specified when KeyType is CUSTOMER_MANAGED_CMK".to_string(),
            ))
        }
        ("AWS_OWNED_CMK", Some(_)) => {
            return Err(EncryptionConfigRefusal::InvalidArgument(
                "KeyARN cannot be specified when KeyType is AWS_OWNED_CMK".to_string(),
            ))
        }
        _ => {}
    }
    // A CMK is reachable only from its own Region, so one named in another Region cannot
    // be used: KMS answers NotFoundException, which Firehose reports as
    // `InvalidKMSResourceException` ("Firehose throws this exception when an attempt to
    // put records or to start or stop Firehose stream encryption fails. This happens when
    // the KMS service throws one of the following exception types: AccessDeniedException,
    // InvalidStateException, DisabledException, or NotFoundException."). The ARN grammar
    // itself is modelled, so the boundary has already checked it; the Region is the part
    // this emulator can judge.
    if let Some(key_arn) = requested.key_arn.as_ref() {
        let key_region = key_arn.split(':').nth(3).unwrap_or_default();
        if !key_region.is_empty() && key_region != region {
            return Err(EncryptionConfigRefusal::InvalidKmsResource {
                code: "NotFoundException".to_string(),
                message: format!(
                    "the KMS key {key_arn} is in Region {key_region} and cannot be used \
                     by a Firehose stream in Region {region}"
                ),
            });
        }
        // And a CMK the stream's own account does not own is one whose key policy this
        // account cannot be granted on: encrypting a stream under a customer managed CMK
        // is Firehose creating a KMS GRANT on that key, and only the key's own account can
        // allow that. KMS answers such a caller AccessDeniedException rather than
        // disclosing whether the key is there at all, so that is the exception type this
        // refusal relays and the sentence it carries is the one about the key policy.
        let key_account = key_arn.split(':').nth(4).unwrap_or_default();
        if !key_account.is_empty() && key_account != ::engine::emulator_account_id() {
            return Err(EncryptionConfigRefusal::InvalidKmsResource {
                code: KMS_ACCESS_DENIED_EXCEPTION.to_string(),
                message: KMS_ACCESS_DENIED.to_string(),
            });
        }
    }
    Ok(())
}

/// Why KMS cannot serve the CMK `requested` names, when it cannot; `None` when the key is
/// one this stream can be encrypted under (and for a request that names no key at all,
/// which asks for the service-account CMK).
///
/// Firehose does not take a `KeyARN` on trust: a stream it reports as encrypted is a stream
/// whose records it can actually wrap, so the key is RESOLVED before the configuration is
/// recorded — and a key KMS does not serve is reported as `InvalidKMSResourceException`
/// ("Firehose throws this exception when an attempt to put records or to start or stop
/// Firehose stream encryption fails. This happens when the KMS service throws one of the
/// following exception types: AccessDeniedException, InvalidStateException,
/// DisabledException, or NotFoundException"), whose `Code` relays the KMS exception type.
///
/// KMS is reached the way this crate reaches IAM and Amazon S3 ([`unassumable_role`]):
/// through the in-process callback, so this crate names no other service crate and no
/// socket is opened. Nothing is refused that could not be grounded in KMS's own answer —
/// with no KMS mounted to ask, nothing is known about the key and the request proceeds.
pub async fn unusable_kms_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    requested: &DeliveryStreamEncryptionConfigurationInput,
) -> Option<EncryptionConfigRefusal> {
    let key_arn = requested.key_arn.as_ref()?;
    match kms_client(ctx, region)
        .describe_key()
        .key_id(key_arn)
        .send()
        .await
    {
        // KMS holds the key: it can wrap this stream's records only while it is enabled.
        // A key in any other state is refused under the exception type KMS itself raises
        // for an operation on a key in that state.
        Ok(described) => {
            let state = described
                .key_metadata()
                .and_then(::aws_sdk_kms::types::KeyMetadata::key_state)
                .map(|state| state.as_str().to_string());
            match state.as_deref() {
                None | Some(KMS_KEY_ENABLED) => None,
                Some(KMS_KEY_DISABLED) => Some(EncryptionConfigRefusal::InvalidKmsResource {
                    code: KMS_DISABLED_EXCEPTION.to_string(),
                    message: unusable_key_message(key_arn, KMS_KEY_DISABLED),
                }),
                Some(other) => Some(EncryptionConfigRefusal::InvalidKmsResource {
                    code: KMS_INVALID_STATE_EXCEPTION.to_string(),
                    message: unusable_key_message(key_arn, other),
                }),
            }
        }
        Err(error) => {
            // KMS denied the call: the key policy does not let this stream's Firehose use
            // the key, which is a grounded refusal and the one Firehose words in terms of
            // that policy ([`KMS_ACCESS_DENIED`]).
            let denied = error.as_service_error().is_some_and(|service_error| {
                ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(service_error)
                    == Some(KMS_ACCESS_DENIED_EXCEPTION)
            });
            if denied {
                return Some(EncryptionConfigRefusal::InvalidKmsResource {
                    code: KMS_ACCESS_DENIED_EXCEPTION.to_string(),
                    message: KMS_ACCESS_DENIED.to_string(),
                });
            }
            // KMS says this account holds no such key: the refusal is grounded, and it
            // carries KMS's own sentence about the key where KMS stated one.
            let not_found = error.as_service_error().is_some_and(
                ::aws_sdk_kms::operation::describe_key::DescribeKeyError::is_not_found_exception,
            );
            if not_found {
                let message = error
                    .as_service_error()
                    .and_then(::aws_smithy_types::error::metadata::ProvideErrorMetadata::message)
                    .map_or_else(
                        || unusable_key_message(key_arn, KMS_NOT_FOUND_EXCEPTION),
                        str::to_string,
                    );
                return Some(EncryptionConfigRefusal::InvalidKmsResource {
                    code: KMS_NOT_FOUND_EXCEPTION.to_string(),
                    message,
                });
            }
            // Any other answer is this emulator failing to ask, not KMS reporting a key it
            // cannot serve, so it refuses nothing.
            ::tracing::warn!(
                kms_key = %key_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the requested CMK could not be resolved against KMS"
            );
            None
        }
    }
}

/// The `KeyState` of a CMK KMS can serve: "Enables or disables the key" — only an enabled
/// key performs cryptographic operations.
const KMS_KEY_ENABLED: &str = "Enabled";
/// The `KeyState` of a CMK its owner turned off.
const KMS_KEY_DISABLED: &str = "Disabled";
/// The KMS exception types Firehose relays in `InvalidKMSResourceException$Code`, for a key
/// KMS does not hold and for one whose state does not admit the operation.
const KMS_NOT_FOUND_EXCEPTION: &str = "NotFoundException";
const KMS_DISABLED_EXCEPTION: &str = "DisabledException";
const KMS_INVALID_STATE_EXCEPTION: &str = "InvalidStateException";
/// The KMS exception type for a call the CMK's key policy does not allow, which is the
/// one Firehose relays for a key it is not permitted to use.
const KMS_ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";

/// What Firehose states for a CMK KMS would not let it use.
///
/// Encrypting a stream under a customer managed CMK is Firehose creating a KMS grant on
/// that key, so a key policy that does not allow the grant APIs is what a caller has to
/// change — and that is what the sentence says. It names no key: the fault is the policy
/// of whichever CMK the request pointed at, and the refusal is the same sentence for all
/// of them.
const KMS_ACCESS_DENIED: &str = "AccessDenied by KMS while using the CMK. Check the key \
     policy to ensure that the KMS grant related apis are allowed. Try again after the key \
     policy has the right permissions.";

/// What Firehose states for a CMK it cannot use: the key the request named and the reason
/// KMS gave for it.
fn unusable_key_message(key_arn: &str, reason: &str) -> String {
    format!("Firehose is unable to use the KMS key {key_arn}: {reason}.")
}

/// The KMS client this service resolves a CMK through: the AWS SDK driven over the
/// emulator's own in-process transport, exactly as [`iam_client`] is.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}

/// The first role of `role_arns` Firehose cannot assume, worded as the refusal states it.
///
/// Firehose does not take a configuration's `RoleARN` on trust: it assumes the role when
/// the stream is written — not when a record is delivered — so a role this account does
/// not hold, or one whose trust policy does not let the Firehose service principal assume
/// it, is an ARGUMENT fault and the stream is never created. Every `RoleARN` the request
/// carries is resolved, whichever configuration (destination, source, VPC, secret,
/// schema) named it: they are all roles Firehose itself assumes.
///
/// IAM is reached the way any emulated service reaches another: through the in-process
/// callback ([`smithy_server_runtime::OperationContext::callback`]), so this crate names
/// no other service crate. Nothing is refused that could not be grounded in IAM's own
/// answer — with no IAM mounted to ask, nothing is known about the role and the request
/// proceeds (worth a warning, because that is a deployment where this gate does not run).
pub async fn unassumable_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arns: &[&str],
) -> Option<String> {
    let iam = iam_client(ctx, region);
    for role_arn in role_arns {
        // A value that names no role of THIS account cannot be assumed, and that is
        // settled without asking IAM: there is nothing of this account to ask about. The
        // reading is the one every service resolving a caller-supplied role shares
        // (`service_common::iam_role`).
        let Some(role_name) = ::service_common::iam_role::role_name(role_arn) else {
            return Some(crate::service::unassumable_role_message(role_arn));
        };
        match iam.get_role().role_name(role_name).send().await {
            Ok(output) => {
                let trusted = output
                    .role()
                    .and_then(::aws_sdk_iam::types::Role::assume_role_policy_document)
                    .is_some_and(trust_policy_admits);
                if !trusted {
                    return Some(crate::service::unassumable_role_message(role_arn));
                }
            }
            // IAM says this account holds no such role: the refusal is grounded.
            Err(error) if role_is_absent(&error) => {
                return Some(crate::service::unassumable_role_message(role_arn))
            }
            // Any other answer is this emulator failing to ask, not IAM reporting a bad
            // role, so it refuses nothing.
            Err(error) => ::tracing::warn!(
                %role_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the configuration's role could not be resolved against IAM"
            ),
        }
    }
    None
}

/// The IAM client this service resolves a role through: the AWS SDK driven over the
/// emulator's own in-process transport.
fn iam_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_iam::Client {
    let config = ::aws_sdk_iam::Config::builder()
        .behavior_version(::aws_sdk_iam::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_iam::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://iam.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_iam::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_iam::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_iam::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_iam::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_iam::Client::from_conf(config)
}

/// Whether an IAM trust-policy document lets Firehose assume the role.
///
/// The rule is IAM's, not this service's, so it is read through the ONE evaluator every
/// service that resolves a caller-supplied role shares
/// (`service_common::iam_trust_policy`): a statement is about the call when it names the
/// principal and the assume-role action, an explicit `Deny` wins over any `Allow`, and the
/// document is percent-decoded before it is parsed.
///
/// A document this emulator cannot read as a policy is not one Firehose can act on, so it
/// does not admit the assume: the role was resolved and carries no grant this can read.
fn trust_policy_admits(document: &str) -> bool {
    ::service_common::iam_trust_policy::admits_service_principal(
        document,
        FIREHOSE_SERVICE_PRINCIPAL,
    )
    .unwrap_or(false)
}

/// Whether an IAM `GetRole` failure is IAM reporting that the role does not exist
/// (`NoSuchEntity`), as opposed to the call not reaching IAM at all.
fn role_is_absent(
    error: &::aws_sdk_iam::error::SdkError<
        ::aws_sdk_iam::operation::get_role::GetRoleError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_iam::operation::get_role::GetRoleError::is_no_such_entity_exception)
}

/// The first bucket of `bucket_arns` Amazon S3 reports it does not hold, worded as the
/// refusal states it.
///
/// A `BucketARN` is not taken on trust any more than a `RoleARN` is: Firehose resolves the
/// Amazon S3 location a destination configuration names before it writes that
/// configuration onto a stream, so a bucket that does not exist is an ARGUMENT fault
/// answered at the call that named it rather than a stream that silently stops delivering.
/// Every `BucketARN` the request carries is resolved, whichever member (the destination's
/// own, a nested S3 configuration's, a backup's) named it: they are all locations Firehose
/// writes objects to.
///
/// Amazon S3 is reached the way any emulated service reaches another: through the
/// in-process callback ([`smithy_server_runtime::OperationContext::callback`]), with
/// `HeadBucket` — the operation S3 documents for exactly this question ("You can use this
/// operation to determine if a bucket exists"). Nothing is refused that could not be
/// grounded in S3's own answer: with no S3 mounted to ask, or on any other failure to ask,
/// nothing is known about the bucket and the request proceeds (worth a warning, because
/// that is a deployment where this gate does not run).
pub async fn absent_destination_bucket(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket_arns: &[&str],
) -> Option<String> {
    let s3 = s3_client(ctx, region);
    // The bucket NAMES already asked about: two members naming one bucket, however their
    // ARNs are spelled, are one question for Amazon S3.
    let mut asked: Vec<&str> = Vec::with_capacity(bucket_arns.len());
    for bucket_arn in bucket_arns {
        // The modelled `BucketARN` pattern (`^arn:.*:s3:::[\w\.\-]{1,255}$`) is checked at
        // the boundary, so a value that reached a handler carries a bucket name; one that
        // somehow carries none names no bucket for this to resolve.
        let Some(bucket) = delivery::bucket_name(bucket_arn) else {
            continue;
        };
        if asked.contains(&bucket) {
            continue;
        }
        asked.push(bucket);
        match s3.head_bucket().bucket(bucket).send().await {
            // S3 holds the bucket, so the destination can deliver to it.
            Ok(_) => {}
            // S3 says this account holds no such bucket: the refusal is grounded.
            Err(error) if bucket_is_absent(&error) => {
                return Some(crate::service::absent_bucket_message(bucket))
            }
            // Any other answer is this emulator failing to ask, not S3 reporting a bad
            // bucket, so it refuses nothing.
            Err(error) => ::tracing::warn!(
                %bucket_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the configuration's bucket could not be resolved against S3"
            ),
        }
    }
    None
}

/// The S3 client this service resolves a destination bucket through: the AWS SDK driven
/// over the emulator's own in-process transport, built exactly like [`iam_client`].
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in the
        // path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether an S3 `HeadBucket` failure is S3 reporting that the bucket does not exist
/// (`NotFound`, the only error that operation models), as opposed to the call not reaching
/// S3 at all.
fn bucket_is_absent(
    error: &::aws_sdk_s3::error::SdkError<
        ::aws_sdk_s3::operation::head_bucket::HeadBucketError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_s3::operation::head_bucket::HeadBucketError::is_not_found)
}

/// `role_arns` with the empty entries dropped and each distinct ARN kept once, so one
/// role named by two members of a configuration is resolved against IAM once.
#[must_use]
pub fn distinct_role_arns(role_arns: Vec<&str>) -> Vec<&str> {
    let mut distinct: Vec<&str> = Vec::with_capacity(role_arns.len());
    for role_arn in role_arns {
        if !role_arn.is_empty() && !distinct.contains(&role_arn) {
            distinct.push(role_arn);
        }
    }
    distinct
}

/// The role a `SecretsManagerConfiguration` names, when it names one.
///
/// Beside [`distinct_role_arns`] because the create and the update paths both read this
/// shape out of their own input, and both resolve the role it names.
#[must_use]
pub fn secrets_manager_role(config: Option<&SecretsManagerConfiguration>) -> Option<&str> {
    config.and_then(|secret| secret.role_arn.as_deref())
}

/// The role a `VpcConfiguration` names — the role Firehose reaches a VPC destination
/// with, which it assumes exactly like the destination's own.
#[must_use]
pub fn vpc_role(config: Option<&VpcConfiguration>) -> Option<&str> {
    config.map(|vpc| vpc.role_arn.as_str())
}

/// The Region a request was signed for, which is the Region a minted ARN names.
///
/// An unsigned request, or one whose credential scope carries a Region the ARN grammar
/// (`[a-zA-Z0-9-]+`, and short enough to keep the ARN inside its modelled 512-character
/// bound) cannot hold, falls back to the emulator's default Region.
///
/// Lives here, beside the state the ARNs are stored in, so every operation that has to
/// mint or match one uses the same rule.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    const DEFAULT_REGION: &str = "us-east-1";
    /// The longest Region token this accepts: every published Region name is far shorter.
    const MAX_REGION_LEN: usize = 32;
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LEN
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFirehoseFirehose20150804State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFirehoseFirehose20150804State>)
}
