// SPDX-License-Identifier: CC0-1.0

//! @op CreateEventSourceMapping
//! Persists ONE event source mapping in [`crate::state`]: the event source the request
//! names, the function it resolves, and every polling, filtering, destination and
//! logging setting the request stated, under a UUID and an ARN minted once. A mapping
//! asked to be active answers `Creating`, and the move to `Enabled` is owed by the CLOCK
//! (see [`crate::state::EventSourceMappingRecord::materialize`]); a mapping asked for
//! with polling paused has no poller to stand up and so is created already `Disabled`
//! (see [`crate::state::created_event_source_mapping_state`]).
//!
//! The event source has to be one `EventSourceArn` names: a resource of one of the
//! vendors and resource types that member enumerates, in this account (see
//! [`crate::state::names_event_source`] and
//! [`crate::state::event_source_is_mappable_by_this_account`]). Anything else is refused
//! with the modelled `InvalidParameterValueException` rather than stored as a mapping of
//! a source nothing could poll — stating the ARN format the source service the value names
//! is polled through where it names one at all, and otherwise that the source ARN sent is
//! unsupported (see [`unrecognized_event_source`]).
//!
//! The two settings the model leaves optional because only SOME of those sources need them
//! — the position a stream is read from, and the instant a position stated as a timestamp
//! starts at — are required for the sources and the position that need them, and a request
//! that leaves one out is refused as the request validation error it is (see
//! [`absent_required_member`]).
//!
//! What the mapping may be asked to MEASURE is held to that same source:
//! `EventSourceMappingMetricsConfig$Metrics` is "the metrics you want your event source
//! mapping to produce", which is a SET of the values its enum declares drawn from the ones
//! that source supports — two of the three are documented for the Kafka event sources alone —
//! so a value that source cannot produce is refused with that same modelled 400 naming the
//! values the source does expect, and a selection that repeats a value is refused with that
//! modelled 400 carrying its own fixed sentence instead, independently of which values the
//! source supports; in either case no mapping is created (see
//! [`crate::state::metrics_config_refusal`]).
//!
//! Where a record of a failed batch is handed to is held to the same source: a destination
//! of `DestinationConfig` is a resource Lambda WRITES that record to, and the kinds
//! `OnFailure$Destination` enumerates are the ones it writes to, so a destination naming a
//! resource of any other service is refused with that same modelled 400 and no mapping is
//! created (see [`crate::state::unsupported_mapping_destination`]) rather than stored as a
//! dead-letter path nothing could ever write to.
//!
//! A mapping on a QUEUE is also held to the queue's own configuration: a queue hands a
//! message to one consumer for the length of its `VisibilityTimeout` and makes it visible
//! again once that lapses, so a queue whose timeout is below the invoked function's
//! `Timeout` would re-deliver a message the function is still working on. That pair is
//! refused with the same modelled 400 (see [`crate::state::queue_visibility_refusal`]),
//! which is why the queue's timeout is read out of its own store here.
//!
//! Every mapping is also held to what the INVOKED FUNCTION'S EXECUTION ROLE may do: Lambda
//! reads the event source acting as that role, so a role the policies in force on it do not
//! let read the named source is refused with that same modelled 400 and no mapping is
//! stored (see [`crate::state::event_source_authorization`]), rather than wired up as a
//! trigger that could never fire.
//!
//! One function reads a given set of records through at most one mapping, so a create
//! that would read records the function is already mapped to is refused with the modelled
//! `ResourceConflictException` ("The resource already exists, or another operation is in
//! progress") and nothing is stored. Which records those are is the source TOGETHER WITH
//! what is consumed inside it — the bootstrap servers for a self-managed Apache Kafka
//! cluster, which carries no ARN, and the topic or the destination queue for a source read
//! per topic or per queue (see [`crate::state::polled_event_source`]).
//!
//! An accepted mapping on a queue also STARTS POLLING it: "Lambda reads items from the
//! event source and invokes the function", so this call arms the poll loop
//! ([`crate::state::drive_sqs_pollers`]) that takes each batch out of the queue and hands
//! it to the mapping's function. Nothing waits for a poll here — the loop runs on its own
//! clock — and what one poll does, including why an unprocessed batch is left on the queue,
//! is documented in [`crate::state::sqs_poller`].
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateEventSourceMapping::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateEventSourceMapping`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")
//! ```
//!
//! Creates a mapping between an event source and an Lambda Event Function. Lambda reads
//! items from the event source and invokes the function. For details about how to configure
//! different event sources, see the following topics. Amazon DynamoDB Streams Amazon
//! Kinesis Amazon SQS Amazon MQ and RabbitMQ Amazon MSK Apache Kafka Amazon DocumentDB The
//! following error handling options are available for stream sources (DynamoDB, Kinesis,
//! Amazon MSK, and self-managed Apache Kafka): BisectBatchOnFunctionError – If the function
//! returns an error, split the batch in two and retry. MaximumRecordAgeInSeconds – Discard
//! records older than the specified age. The default value is infinite (-1). When set to
//! infinite (-1), failed records are retried until the record expires MaximumRetryAttempts
//! – Discard records after the specified number of retries. The default value is infinite
//! (-1). When set to infinite (-1), failed records are retried until the record expires.
//! OnFailure – Send discarded records to an Amazon SQS queue, Amazon SNS topic, Kafka
//! topic, or Amazon S3 bucket. For more information, see Adding a destination. The
//! following option is available only for DynamoDB and Kinesis event sources:
//! ParallelizationFactor – Process multiple batches from each shard concurrently. For
//! information about which configuration parameters apply to each event source, see the
//! following topics. Amazon DynamoDB Streams Amazon Kinesis Amazon SQS Amazon MQ and
//! RabbitMQ Amazon MSK Apache Kafka Amazon DocumentDB
//!
//! ## Input
//!
//! * `AmazonManagedKafkaEventSourceConfig` (AmazonManagedKafkaEventSourceConfig) — Specific
//!   configuration settings for an Amazon Managed Streaming for Apache Kafka (Amazon MSK)
//!   event source.
//! * `BatchSize` (BatchSize, range 1..10000) — The maximum number of records in each batch
//!   that Lambda pulls from your stream or queue and sends to your function. Lambda passes
//!   all of the records in the batch to the function in a single call, up to the payload
//!   limit for synchronous invocation (6 MB). Amazon Kinesis – Default 100. Max 10,000.
//!   Amazon DynamoDB Streams – Default 100. Max 10,000. Amazon Simple Queue Service – Default
//!   10. For standard queues the max is 10,000. For FIFO queues the max is 10. Amazon Managed
//!   Streaming for Apache Kafka – Default 100. Max 10,000. Self-managed Apache Kafka –
//!   Default 100. Max 10,000. Amazon MQ (ActiveMQ and RabbitMQ) – Default 100. Max 10,000.
//!   DocumentDB – Default 100. Max 10,000.
//! * `BisectBatchOnFunctionError` (BisectBatchOnFunctionError) — (Kinesis, DynamoDB Streams,
//!   Amazon MSK, and self-managed Apache Kafka) If the function returns an error, split the
//!   batch in two and retry.
//! * `DestinationConfig` (DestinationConfig) — (Kinesis, DynamoDB Streams, Amazon MSK, and
//!   self-managed Apache Kafka) A configuration object that specifies the destination of an
//!   event after Lambda processes it.
//! * `DocumentDBEventSourceConfig` (DocumentDBEventSourceConfig) — Specific configuration
//!   settings for a DocumentDB event source.
//! * `Enabled` (Enabled) — When true, the event source mapping is active. When false, Lambda
//!   pauses polling and invocation. Default: True
//! * `EventSourceArn` (Arn, len 0..10000, pattern
//!   ^arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-])+:((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1})?:(\d{12})?:(.*)$)
//!   — The Amazon Resource Name (ARN) of the event source. Amazon Kinesis – The ARN of the
//!   data stream or a stream consumer. Amazon DynamoDB Streams – The ARN of the stream.
//!   Amazon Simple Queue Service – The ARN of the queue. Amazon Managed Streaming for Apache
//!   Kafka – The ARN of the cluster or the ARN of the VPC connection (for cross-account event
//!   source mappings). Amazon MQ – The ARN of the broker. Amazon DocumentDB – The ARN of the
//!   DocumentDB change stream.
//! * `FilterCriteria` (FilterCriteria) — An object that defines the filter criteria that
//!   determine whether Lambda should process an event. For more information, see Lambda event
//!   filtering.
//! * `FunctionName` (NamespacedFunctionName, len 1..256, pattern
//!   ^(arn:(aws[a-zA-Z-]*)?:lambda:)?((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:)?(\d{12}:)?(function:)?([a-zA-Z0
//!   …) **required** — The name or ARN of the Lambda function. Name formats Function name –
//!   MyFunction. Function ARN – arn:aws:lambda:us-west-2:123456789012:function:MyFunction.
//!   Version or Alias ARN – arn:aws:lambda:us-west-2:123456789012:function:MyFunction:PROD.
//!   Partial ARN – 123456789012:function:MyFunction. The length constraint applies only to
//!   the full ARN. If you specify only the function name, it's limited to 64 characters in
//!   length.
//! * `FunctionResponseTypes` (FunctionResponseTypeList, len 0..1) — (Kinesis, DynamoDB
//!   Streams, Amazon MSK, self-managed Apache Kafka, and Amazon SQS) A list of current
//!   response type enums applied to the event source mapping.
//! * `KMSKeyArn` (KMSKeyArn, len 0..10000, pattern
//!   ^(arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*)|()$) — The ARN of the Key Management Service
//!   (KMS) customer managed key that Lambda uses to encrypt your function's filter criteria.
//!   By default, Lambda does not encrypt your filter criteria object. Specify this property
//!   to encrypt data using your own customer managed key.
//! * `LoggingConfig` (EventSourceMappingLoggingConfig) — (Amazon MSK, and self-managed Apache
//!   Kafka only) The logging configuration for your event source. For more information, see
//!   Event source mapping logging.
//! * `MaximumBatchingWindowInSeconds` (MaximumBatchingWindowInSeconds, range 0..300) — The
//!   maximum amount of time, in seconds, that Lambda spends gathering records before invoking
//!   the function. You can configure MaximumBatchingWindowInSeconds to any value from 0
//!   seconds to 300 seconds in increments of seconds. For Kinesis, DynamoDB, and Amazon SQS
//!   event sources, the default batching window is 0 seconds. For Amazon MSK, Self-managed
//!   Apache Kafka, Amazon MQ, and DocumentDB event sources, the default batching window is
//!   500 ms. Note that because you can only change MaximumBatchingWindowInSeconds in
//!   increments of seconds, you cannot revert back to the 500 ms default batching window
//!   after you have changed it. To restore the default batching window, you must create a new
//!   event …
//! * `MaximumRecordAgeInSeconds` (MaximumRecordAgeInSeconds, range -1..604800) — (Kinesis,
//!   DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records older than
//!   the specified age. The default value is infinite (-1).
//! * `MaximumRetryAttempts` (MaximumRetryAttemptsEventSourceMapping, range -1..10000) —
//!   (Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records
//!   after the specified number of retries. The default value is infinite (-1). When set to
//!   infinite (-1), failed records are retried until the record expires.
//! * `MetricsConfig` (EventSourceMappingMetricsConfig) — The metrics configuration for your
//!   event source. For more information, see Event source mapping metrics.
//! * `ParallelizationFactor` (ParallelizationFactor, range 1..10) — (Kinesis and DynamoDB
//!   Streams only) The number of batches to process from each shard concurrently.
//! * `Queues` (Queues, len 1..1) — (MQ) The name of the Amazon MQ broker destination queue to
//!   consume.
//! * `Tags` (Tags) — A list of tags to apply to the event source mapping.
//! * +9 more omitted for length; get all of them with:
//!   smithy_query(query="shape",shape="com.amazonaws.lambda#CreateEventSourceMappingRequest",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")
//!
//! ## Output
//!
//! * `AmazonManagedKafkaEventSourceConfig` (AmazonManagedKafkaEventSourceConfig) — Specific
//!   configuration settings for an Amazon Managed Streaming for Apache Kafka (Amazon MSK)
//!   event source.
//! * `BatchSize` (BatchSize, range 1..10000) — The maximum number of records in each batch
//!   that Lambda pulls from your stream or queue and sends to your function. Lambda passes
//!   all of the records in the batch to the function in a single call, up to the payload
//!   limit for synchronous invocation (6 MB). Default value: Varies by service. For Amazon
//!   SQS, the default is 10. For all other services, the default is 100. Related setting:
//!   When you set BatchSize to a value greater than 10, you must set
//!   MaximumBatchingWindowInSeconds to at least 1.
//! * `BisectBatchOnFunctionError` (BisectBatchOnFunctionError) — (Kinesis, DynamoDB Streams,
//!   Amazon MSK, and self-managed Apache Kafka) If the function returns an error, split the
//!   batch in two and retry. The default value is false.
//! * `DestinationConfig` (DestinationConfig) — (Kinesis, DynamoDB Streams, Amazon MSK, and
//!   self-managed Apache Kafka) A configuration object that specifies the destination of an
//!   event after Lambda processes it.
//! * `DocumentDBEventSourceConfig` (DocumentDBEventSourceConfig) — Specific configuration
//!   settings for a DocumentDB event source.
//! * `EventSourceArn` (Arn, len 0..10000, pattern
//!   ^arn:(aws[a-zA-Z0-9-]*):([a-zA-Z0-9\-])+:((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1})?:(\d{12})?:(.*)$)
//!   — The Amazon Resource Name (ARN) of the event source.
//! * `EventSourceMappingArn` (EventSourceMappingArn, len 85..120, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:event-source-mapping:[0-9a-fA-
//!   …) — The Amazon Resource Name (ARN) of the event source mapping.
//! * `FilterCriteria` (FilterCriteria) — An object that defines the filter criteria that
//!   determine whether Lambda should process an event. For more information, see Lambda event
//!   filtering. If filter criteria is encrypted, this field shows up as null in the response
//!   of ListEventSourceMapping API calls. You can view this field in plaintext in the
//!   response of GetEventSourceMapping and DeleteEventSourceMapping calls if you have
//!   kms:Decrypt permissions for the correct KMS key.
//! * `FilterCriteriaError` (FilterCriteriaError) — An object that contains details about an
//!   error related to filter criteria encryption.
//! * `FunctionArn` (FunctionArn, len 0..10000, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:function:[a-zA-Z0-9-_]+(:(\$LA
//!   …) — The ARN of the Lambda function.
//! * `FunctionResponseTypes` (FunctionResponseTypeList, len 0..1) — (Kinesis, DynamoDB
//!   Streams, Amazon MSK, self-managed Apache Kafka, and Amazon SQS) A list of current
//!   response type enums applied to the event source mapping.
//! * `KMSKeyArn` (KMSKeyArn, len 0..10000, pattern
//!   ^(arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*)|()$) — The ARN of the Key Management Service
//!   (KMS) customer managed key that Lambda uses to encrypt your function's filter criteria.
//! * `LastModified` (Date) — The date that the event source mapping was last updated or that
//!   its state changed.
//! * `LastProcessingResult` (String) — The result of the event source mapping's last
//!   processing attempt.
//! * `LoggingConfig` (EventSourceMappingLoggingConfig) — (Amazon MSK, and self-managed Apache
//!   Kafka only) The logging configuration for your event source. For more information, see
//!   Event source mapping logging.
//! * `MaximumBatchingWindowInSeconds` (MaximumBatchingWindowInSeconds, range 0..300) — The
//!   maximum amount of time, in seconds, that Lambda spends gathering records before invoking
//!   the function. You can configure MaximumBatchingWindowInSeconds to any value from 0
//!   seconds to 300 seconds in increments of seconds. For streams and Amazon SQS event
//!   sources, the default batching window is 0 seconds. For Amazon MSK, Self-managed Apache
//!   Kafka, Amazon MQ, and DocumentDB event sources, the default batching window is 500 ms.
//!   Note that because you can only change MaximumBatchingWindowInSeconds in increments of
//!   seconds, you cannot revert back to the 500 ms default batching window after you have
//!   changed it. To restore the default batching window, you must create a new event source
//!   mapping. …
//! * `MaximumRecordAgeInSeconds` (MaximumRecordAgeInSeconds, range -1..604800) — (Kinesis,
//!   DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records older than
//!   the specified age. The default value is -1, which sets the maximum age to infinite. When
//!   the value is set to infinite, Lambda never discards old records. The minimum valid value
//!   for maximum record age is 60s. Although values less than 60 and greater than -1 fall
//!   within the parameter's absolute range, they are not allowed
//! * `MaximumRetryAttempts` (MaximumRetryAttemptsEventSourceMapping, range -1..10000) —
//!   (Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records
//!   after the specified number of retries. The default value is -1, which sets the maximum
//!   number of retries to infinite. When MaximumRetryAttempts is infinite, Lambda retries
//!   failed records until the record expires in the event source.
//! * `MetricsConfig` (EventSourceMappingMetricsConfig) — The metrics configuration for your
//!   event source. For more information, see Event source mapping metrics.
//! * `ParallelizationFactor` (ParallelizationFactor, range 1..10) — (Kinesis and DynamoDB
//!   Streams only) The number of batches to process concurrently from each shard. The default
//!   value is 1.
//! * `ProvisionedPollerConfig` (ProvisionedPollerConfig) — (Amazon SQS, Amazon MSK, and
//!   self-managed Apache Kafka only) The provisioned mode configuration for the event source.
//!   For more information, see provisioned mode.
//! * `Queues` (Queues, len 1..1) — (Amazon MQ) The name of the Amazon MQ broker destination
//!   queue to consume.
//! * `ScalingConfig` (ScalingConfig) — (Amazon SQS only) The scaling configuration for the
//!   event source. For more information, see Configuring maximum concurrency for Amazon SQS
//!   event sources.
//! * `SelfManagedEventSource` (SelfManagedEventSource) — The self-managed Apache Kafka
//!   cluster for your event source.
//! * `SelfManagedKafkaEventSourceConfig` (SelfManagedKafkaEventSourceConfig) — Specific
//!   configuration settings for a self-managed Apache Kafka event source.
//! * `SourceAccessConfigurations` (SourceAccessConfigurations, len 0..23) — An array of the
//!   authentication protocol, VPC components, or virtual host to secure and define your event
//!   source.
//! * `StartingPosition` (EventSourcePosition, one of: AT_TIMESTAMP | LATEST | TRIM_HORIZON) —
//!   The position in a stream from which to start reading. Required for Amazon Kinesis and
//!   Amazon DynamoDB Stream event sources. AT_TIMESTAMP is supported only for Amazon Kinesis
//!   streams, Amazon DocumentDB, Amazon MSK, and self-managed Apache Kafka.
//! * `StartingPositionTimestamp` (Date) — With StartingPosition set to AT_TIMESTAMP, the time
//!   from which to start reading. StartingPositionTimestamp cannot be in the future.
//! * `State` (String) — The state of the event source mapping. It can be one of the
//!   following: Creating, Enabling, Enabled, Disabling, Disabled, Updating, or Deleting.
//! * `StateTransitionReason` (String) — Indicates whether a user or Lambda made the last
//!   change to the event source mapping.
//! * `Topics` (Topics, len 1..1) — The name of the Kafka topic.
//! * `TumblingWindowInSeconds` (TumblingWindowInSeconds, range 0..900) — (Kinesis and
//!   DynamoDB Streams only) The duration in seconds of a processing window for DynamoDB and
//!   Kinesis Streams event sources. A value of 0 seconds indicates no tumbling window.
//! * `UUID` (UUIDString, len 36..36) — The identifier of the event source mapping.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — One of the parameters in the request is
//!   not valid.
//! * `ResourceConflictException` (HTTP 409) — The resource already exists, or another
//!   operation is in progress.
//! * `ResourceNotFoundException` (HTTP 404) — The resource specified in the request does not
//!   exist.
//! * `ServiceException` (HTTP 500) — The Lambda service encountered an internal error.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was exceeded. For
//!   more information, see Lambda quotas.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEventSourceMappingError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEventSourceMappingError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.lambda#CreateEventSourceMapping")]
#[derive(Debug)]
pub enum CreateEventSourceMappingErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceConflictException(ResourceConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_event_source_mapping::CreateEventSourceMappingError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEventSourceMappingError = crate::scaffolded::Scaffolded<CreateEventSourceMappingErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEventSourceMappingErrorModeled> for CreateEventSourceMappingError {
    fn from(modeled: CreateEventSourceMappingErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEventSourceMapping` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEventSourceMapping<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEventSourceMapping<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEventSourceMapping<Eng>
{
    type Input = CreateEventSourceMappingInput;
    type Output = CreateEventSourceMappingOutput;
    type Error = CreateEventSourceMappingError;

    const NAME: &'static str = "CreateEventSourceMapping";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_EVENT_SOURCE_MAPPING_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_EVENT_SOURCE_MAPPING_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateEventSourceMappingErrorModeled::ServiceException(
                ServiceException {
                    r#type: None,
                    message: Some("Lambda state is not reachable on this engine".to_string()),
                },
            )
            .into());
        };

        // The tags of the request are the ones the new mapping will carry, so they are
        // judged by the rule a stored tag has to meet before anything is created — a rule
        // whose reserved-namespace half binds a CALLER, so which path the request arrived
        // on is part of it ([`crate::state::tag_key_origin`]).
        if !crate::state::tag_keys_are_storable_from(
            input
                .tags
                .iter()
                .flat_map(|tags| tags.keys().map(String::as_str)),
            crate::state::tag_key_origin(ctx),
        ) {
            return Err(invalid_parameter(
                crate::state::INVALID_TAGS_MESSAGE.to_string(),
            ));
        }
        // What remains is the SHAPE of each key, which the request walk never checks a
        // map's key against ([`crate::state::tag_key_shape_refusal`]).
        if let Some(message) = crate::state::tag_key_shape_refusal(
            input
                .tags
                .iter()
                .flat_map(|tags| tags.iter().map(|(key, value)| (key.as_str(), value.as_str()))),
        ) {
            return Err(crate::scaffolded::Scaffolded::validation(message));
        }

        // "Creates a mapping between an event source and an Lambda Event Function." The
        // event source is either a resource ARN (`EventSourceArn`) or a self-managed
        // Apache Kafka cluster (`SelfManagedEventSource`: "The self-managed Apache Kafka
        // cluster for your event source"), so one mapping names exactly one of them.
        let event_source_arn = input.event_source_arn.clone().filter(|arn| !arn.is_empty());
        let self_managed = input.self_managed_event_source.is_some();
        if event_source_arn.is_some() == self_managed {
            return Err(invalid_parameter(
                "Exactly one of EventSourceArn and SelfManagedEventSource must be specified \
                 to name the event source of the mapping"
                    .to_string(),
            ));
        }

        // The Region the caller signed for: the one this account's resources live in, so
        // it is the Region the ARNs this call mints carry and the one the ARN format a
        // refusal states is written for.
        let region = crate::state::request_region(ctx);

        // Each entry of that enumeration is a resource of ONE vendor namespace and ONE
        // resource type, in the account doing the mapping, so an ARN naming anything else
        // names nothing this mapping could poll however well formed it is.
        if let Some(event_source) = event_source_arn.as_deref() {
            let Some(parsed) = ::service_common::arn::parse(event_source) else {
                return Err(invalid_parameter(format!(
                    "Value '{event_source}' at 'eventSourceArn' failed to satisfy constraint: \
                     Member must be the Amazon Resource Name of an event source"
                )));
            };
            if !crate::state::names_event_source(&parsed) {
                return Err(invalid_parameter(unrecognized_event_source(
                    parsed.vendor,
                    event_source,
                    &region,
                )));
            }
            if !crate::state::event_source_is_mappable_by_this_account(&parsed) {
                return Err(invalid_parameter(format!(
                    "The event source '{event_source}' belongs to account '{}' rather than to \
                     account '{}', which this request is made in",
                    parsed.account,
                    ::engine::emulator_account_id()
                )));
            }
        }

        // Which service the event source belongs to is what the per-source defaults and
        // requirements the model documents are keyed on.
        let vendor = event_source_arn
            .as_deref()
            .and_then(crate::state::event_source_vendor)
            .map(str::to_string);
        let vendor = vendor.as_deref();
        let shard_source = crate::state::is_shard_source(vendor);
        let stream_source = crate::state::is_stream_source(vendor, self_managed);

        let now = ::std::time::SystemTime::now();

        // "The metrics you want your event source mapping to produce" is a SET of the
        // values the enum declares drawn from the ones the source this mapping would poll
        // supports: two of the three are documented for the Kafka event sources alone, and a
        // value named twice is no selection either and carries its own sentence (see
        // [`crate::state::metrics_config_refusal`]).
        if let Some(message) = crate::state::metrics_config_refusal(
            input.metrics_config.as_ref(),
            vendor,
            self_managed,
        ) {
            return Err(invalid_parameter(message));
        }

        // A destination of the mapping is a resource LAMBDA WRITES a record of a failed
        // batch to, and only the kinds `OnFailure$Destination` enumerates are such a
        // resource: a destination naming any other service is a dead-letter path nothing
        // could ever write to, so it is refused here rather than stored as one (see
        // [`crate::state::unsupported_mapping_destination`]).
        if let Some(message) =
            crate::state::unsupported_mapping_destination(input.destination_config.as_ref())
        {
            return Err(invalid_parameter(message));
        }

        // `StartingPosition` is "Required for Amazon Kinesis and Amazon DynamoDB Stream
        // event sources".
        if shard_source && input.starting_position.is_none() {
            return Err(invalid_parameter(absent_required_member(
                STARTING_POSITION_MEMBER,
            )));
        }
        if input.starting_position.as_deref() == Some(crate::state::STARTING_POSITION_AT_TIMESTAMP)
        {
            // "AT_TIMESTAMP is supported only for Amazon Kinesis streams, Amazon
            // DocumentDB, Amazon MSK, and self-managed Apache Kafka."
            if !crate::state::supports_at_timestamp(vendor, self_managed) {
                return Err(invalid_parameter(format!(
                    "StartingPosition AT_TIMESTAMP is not supported for the event source '{}'",
                    event_source_arn.as_deref().unwrap_or_default()
                )));
            }
            // "With StartingPosition set to AT_TIMESTAMP, the time from which to start
            // reading. StartingPositionTimestamp cannot be in the future."
            let Some(timestamp) = input.starting_position_timestamp else {
                return Err(invalid_parameter(absent_required_member(
                    STARTING_POSITION_TIMESTAMP_MEMBER,
                )));
            };
            if timestamp.as_secs_f64() > ::aws_smithy_types::DateTime::from(now).as_secs_f64() {
                return Err(invalid_parameter(
                    "StartingPositionTimestamp cannot be in the future".to_string(),
                ));
            }
        }

        // An Amazon DocumentDB change stream is consumed INSIDE one database of the
        // cluster — `DocumentDBEventSourceConfig$DatabaseName` is "The name of the database
        // to consume within the DocumentDB cluster" — so a mapping onto such a cluster that
        // names no database names nothing to read. The member is optional in the MODEL,
        // because one operation serves every source, so the requirement is stated here; it
        // is a statement about the REQUEST, decided before the source is resolved or the
        // execution role is asked about at all.
        if vendor == Some(crate::state::EVENT_SOURCE_VENDOR_DOCUMENT_DB)
            && input
                .document_db_event_source_config
                .as_ref()
                .and_then(|config| config.database_name.as_deref())
                .is_none_or(str::is_empty)
        {
            return Err(invalid_parameter(absent_required_nested_member(
                DOCUMENT_DB_EVENT_SOURCE_CONFIG_MEMBER,
                DATABASE_NAME_MEMBER,
            )));
        }

        let mut store = state.lock();
        // Every mapping this store reports is first brought up to whatever the clock has
        // already made due, so a mapping whose deletion is due is gone before this call
        // looks for a conflict with it.
        store.materialize_all(now);

        // The mapping invokes one function, so the `FunctionName` it names — in any of
        // its documented forms, qualified or not — has to resolve to a record this store
        // holds.
        let (function, version) = match crate::state::resolve_function(
            &mut store,
            &input.function_name,
            None,
            &region,
            now,
        ) {
            Ok(resolved) => resolved,
            Err(crate::state::ResolveFailure::Malformed(message)) => {
                return Err(invalid_parameter(message))
            }
            Err(crate::state::ResolveFailure::NotFound(message)) => {
                return Err(
                    CreateEventSourceMappingErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            r#type: None,
                            message: Some(message),
                        },
                    )
                    .into(),
                )
            }
        };
        let function_name = function.config.name.clone();
        let function_arn = function.versioned_arn(&version);
        // The `Timeout` of the version this mapping invokes: "The amount of time in
        // seconds that Lambda allows a function to run before stopping it." The resolution
        // above refuses a version this store holds no configuration for, so a version with
        // no timeout to compare is a failure of the store rather than of the request. The
        // same configuration carries the EXECUTION ROLE the mapping's poller would read the
        // event source as.
        let Some((function_timeout, function_role)) = function
            .config_of(&version)
            .map(|config| (config.timeout, config.role.clone()))
        else {
            return Err(CreateEventSourceMappingErrorModeled::ServiceException(
                ServiceException {
                    r#type: None,
                    message: Some(format!(
                        "Lambda resolved {function_arn} without a configuration to read its \
                         timeout from"
                    )),
                },
            )
            .into());
        };
        // `$LATEST` is the function itself rather than one version or alias of it, so a
        // reference to it is an unqualified reference.
        let function_qualifier = (version != crate::state::VERSION_LATEST).then(|| version.clone());

        // An Amazon SQS queue hands a message to one consumer for the length of its
        // visibility timeout, so the timeout of the queue this mapping would poll is what
        // the function's own is compared against. It is read from the QUEUE's own store,
        // because it is the queue's configuration rather than anything the request states.
        let queue_visibility_timeout = event_source_arn
            .as_deref()
            .filter(|_| vendor == Some(crate::state::EVENT_SOURCE_VENDOR_QUEUE))
            .and_then(|arn| crate::state::queue_visibility_timeout_seconds(&*self.engine, arn));

        // The queue would make a message visible to a second poller while this function is
        // still working on it, so the mapping that would do it is refused here rather than
        // leaving the duplicate delivery to surface later.
        if let Some(message) = queue_visibility_timeout.and_then(|visibility_timeout| {
            crate::state::queue_visibility_refusal(visibility_timeout, function_timeout)
        }) {
            return Err(invalid_parameter(message));
        }

        // Lambda reads the event source ACTING AS the function's execution role, so a role
        // the policies in force on it do not let read this source is a trigger that could
        // never fire: it is refused here, before anything is persisted, rather than stood
        // up as a mapping that polls nothing. The role is the one the VERSION this mapping
        // invokes carries, and the decision is IAM's own over its own store — see
        // [`crate::state::event_source_authorization`], including the cases that show
        // nothing about the role and refuse nothing.
        if let Some(message) = event_source_arn.as_deref().and_then(|event_source| {
            crate::state::unreadable_event_source(&*self.engine, &function_role, event_source)
        }) {
            return Err(invalid_parameter(message));
        }

        // "The resource already exists, or another operation is in progress": the records
        // a mapping reads are polled for one function by at most one mapping, so a second
        // create that would read the SAME records for the same function conflicts rather
        // than standing up a second poller over them. Which records those are is the
        // source together with what is consumed inside it — the cluster's bootstrap
        // servers for a self-managed Apache Kafka source, which carries no ARN at all, and
        // the topic or the queue for a source that is read per topic or per queue (see
        // [`crate::state::polled_event_source`]).
        let polled = crate::state::polled_event_source(
            event_source_arn.as_deref(),
            input.self_managed_event_source.as_ref(),
            input.topics.as_deref(),
            input.queues.as_deref(),
        );
        if let Some(polled) = polled.as_ref() {
            if let Some(existing) = store.event_source_mappings.values().find(|mapping| {
                mapping.function_arn == function_arn
                    && crate::state::polled_event_source(
                        mapping.event_source_arn.as_deref(),
                        mapping.self_managed_event_source.as_ref(),
                        mapping.topics.as_deref(),
                        mapping.queues.as_deref(),
                    )
                    .as_ref()
                        == Some(polled)
            }) {
                return Err(
                    CreateEventSourceMappingErrorModeled::ResourceConflictException(
                        ResourceConflictException {
                            r#type: None,
                            // The refusal names what collided — the event source, what is
                            // consumed inside it and the function — beside the UUID of
                            // the mapping that already holds it, which is the one a
                            // caller updates or deletes to converge.
                            message: Some(polled.duplicate_message(
                                &existing.function_arn,
                                &existing.function_name,
                                &existing.uuid,
                            )),
                        },
                    )
                    .into(),
                );
            }
        }

        // The store is in memory, so the slice refuses past its own ceiling rather than
        // growing without bound. The model declares no limit error for this operation, so
        // the refusal is the modelled throttle — what Lambda answers a request that
        // exceeds an account quota with.
        if store.event_source_mappings.len() >= crate::state::MAX_EVENT_SOURCE_MAPPINGS {
            return Err(
                CreateEventSourceMappingErrorModeled::TooManyRequestsException(
                    TooManyRequestsException {
                        retry_after_seconds: None,
                        r#type: None,
                        reason: None,
                        message: Some(format!(
                            "This account already holds the maximum of {} event source mappings.",
                            crate::state::MAX_EVENT_SOURCE_MAPPINGS
                        )),
                    },
                )
                .into(),
            );
        }

        let uuid = crate::state::mint_event_source_mapping_uuid();
        // "When true, the event source mapping is active. When false, Lambda pauses
        // polling and invocation. Default: True" — read once, so the setting the mapping
        // is stored with and the state it is created in cannot disagree.
        let enabled = input.enabled.unwrap_or(true);
        let record = crate::state::EventSourceMappingRecord {
            arn: crate::state::event_source_mapping_arn(&region, &uuid),
            uuid: uuid.clone(),
            function_arn,
            function_name,
            function_qualifier,
            event_source_arn,
            enabled,
            batch_size: input
                .batch_size
                .unwrap_or_else(|| crate::state::default_batch_size(vendor)),
            maximum_batching_window_in_seconds: input
                .maximum_batching_window_in_seconds
                .or_else(|| crate::state::default_batching_window_seconds(vendor)),
            // "(Kinesis and DynamoDB Streams only) The number of batches to process
            // concurrently from each shard. The default value is 1."
            parallelization_factor: input
                .parallelization_factor
                .or_else(|| shard_source.then_some(crate::state::DEFAULT_PARALLELIZATION_FACTOR)),
            starting_position: input.starting_position.clone(),
            starting_position_timestamp: input.starting_position_timestamp,
            filter_criteria: input.filter_criteria.clone(),
            kms_key_arn: input.kms_key_arn.clone(),
            metrics_config: input.metrics_config.clone(),
            logging_config: input.logging_config.clone(),
            scaling_config: input.scaling_config.clone(),
            destination_config: input.destination_config.clone(),
            // The error-handling options the model scopes to stream sources, each with
            // the default its own member documents: "The default value is -1, which sets
            // the maximum age to infinite", the same for the retries, and "If the
            // function returns an error, split the batch in two and retry. The default
            // value is false."
            maximum_record_age_in_seconds: input
                .maximum_record_age_in_seconds
                .or_else(|| stream_source.then_some(crate::state::INFINITE_RETENTION)),
            maximum_retry_attempts: input
                .maximum_retry_attempts
                .or_else(|| stream_source.then_some(crate::state::INFINITE_RETENTION)),
            bisect_batch_on_function_error: input
                .bisect_batch_on_function_error
                .or_else(|| stream_source.then_some(false)),
            // "(Kinesis and DynamoDB Streams only) The duration in seconds of a
            // processing window for DynamoDB and Kinesis Streams event sources. A value of
            // 0 seconds indicates no tumbling window": a mapping on one of those two
            // sources whose request stated no window has NO tumbling window, which is that
            // duration rather than an unknown one, and no window at all is what the member
            // means for every other source.
            tumbling_window_in_seconds: input
                .tumbling_window_in_seconds
                .or_else(|| shard_source.then_some(NO_TUMBLING_WINDOW)),
            topics: input.topics.clone(),
            queues: input.queues.clone(),
            source_access_configurations: input.source_access_configurations.clone(),
            self_managed_event_source: input.self_managed_event_source.clone(),
            // The response types applied to a mapping whose request named none: none of
            // them, which is an empty list rather than an unknown one.
            function_response_types: input.function_response_types.clone().unwrap_or_default(),
            amazon_managed_kafka_event_source_config: input
                .amazon_managed_kafka_event_source_config
                .clone(),
            self_managed_kafka_event_source_config: input
                .self_managed_kafka_event_source_config
                .clone(),
            document_db_event_source_config: input.document_db_event_source_config.clone(),
            provisioned_poller_config: input.provisioned_poller_config.clone(),
            tags: input
                .tags
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            // A mapping asked to be active only has its poller STARTED here — the
            // operation answers 202 Accepted — so it is `Creating` and the move to
            // `Enabled` is owed by the clock at `settled_due`; nothing waits for it. A
            // mapping asked for with polling paused has no poller to stand up, so it is
            // created already `Disabled` (see
            // [`crate::state::created_event_source_mapping_state`]).
            state: crate::state::created_event_source_mapping_state(enabled).to_string(),
            state_transition_reason: crate::state::EVENT_SOURCE_MAPPING_USER_INITIATED.to_string(),
            settled_due: now + crate::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY,
            // Nobody has deleted this mapping, so no removal is owed.
            removal_due: None,
            last_modified: now,
            // `StartingPosition` is "The position in a stream from which to start
            // reading" — a property of the MAPPING — so the instant this call fixes it at
            // is stored with it: a `LATEST` mapping reads what arrives from here on,
            // whether or not it is enabled yet (see
            // [`crate::state::EventSourceMappingRecord::initial_shard_cursor`]).
            created_at: now,
            // Nothing has been read from any shard yet, so every shard is read from the
            // position above.
            shard_cursors: ::std::collections::BTreeMap::new(),
            // "The result of the event source mapping's last processing attempt": this
            // mapping's poller has not had a batch to attempt yet.
            last_processing_result: None,
        };
        store.event_source_mappings.insert(uuid.clone(), record);

        // Answered by reading the record back out of the store, so
        // `GetEventSourceMapping` reports exactly what this call persisted.
        let Some(stored) = store.event_source_mappings.get(&uuid) else {
            return Err(CreateEventSourceMappingErrorModeled::ServiceException(
                ServiceException {
                    r#type: None,
                    message: Some(
                        "The created event source mapping could not be read back".to_string(),
                    ),
                },
            )
            .into());
        };
        let reported = stored.reported();

        // Standing the mapping's poller up is what this call started: "Lambda reads items
        // from the event source and invokes the function", and the reading is the poller's.
        // Which poller depends on the kind of source — the queue loop, or this mapping's own
        // reader of a shard source's shards — and each decides for itself whether it
        // has anything to do. The store's lock is released first (each poller takes it
        // itself) and nothing here waits for a poll: they run on their own clock.
        drop(store);
        crate::state::drive_sqs_pollers(ctx, &self.engine);
        crate::state::event_source_poller::drive_mapping(ctx, &self.engine, &uuid);

        Ok(reported)
    }
}

/// `TumblingWindowInSeconds` for a mapping on one of the two sources that member applies
/// to whose request stated none: "A value of 0 seconds indicates no tumbling window", and
/// a mapping asked for without one HAS no tumbling window.
const NO_TUMBLING_WINDOW: i32 = 0;

/// The Smithy member names of the two settings this operation requires for the source and
/// the position the request names, which the model leaves optional on the input: naming a
/// member by the model's own name is what keeps the path a refusal states tied to the
/// model.
const STARTING_POSITION_MEMBER: &str = "StartingPosition";
/// See [`STARTING_POSITION_MEMBER`].
const STARTING_POSITION_TIMESTAMP_MEMBER: &str = "StartingPositionTimestamp";
/// See [`STARTING_POSITION_MEMBER`]: the structure a DocumentDB mapping's own settings
/// arrive in, and the setting inside it that such a mapping requires.
const DOCUMENT_DB_EVENT_SOURCE_CONFIG_MEMBER: &str = "DocumentDBEventSourceConfig";
/// See [`DOCUMENT_DB_EVENT_SOURCE_CONFIG_MEMBER`].
const DATABASE_NAME_MEMBER: &str = "DatabaseName";

/// The account field of an ARN FORMAT: the twelve-digit example id an ARN written as a
/// SHAPE carries, beside the placeholder names of its resource half, in every format AWS
/// spells out. It is the account half of an example rather than an account anything lives
/// in — which is why it is stated here instead of the account whose resources this emulator
/// holds ([`::engine::emulator_account_id`], the id every ARN this service MINTS carries).
const EXAMPLE_ACCOUNT_ID: &str = "123456789012";

/// How this operation refuses a request that left out a member it requires for the event
/// source or the starting position that request names.
///
/// Such a member is optional in the MODEL — it is required only for some of the sources the
/// one operation serves — so nothing in the request walk can refuse it, and the refusal is
/// still a statement about the REQUEST: AWS's request-validation envelope over the absent
/// member, naming it by the lowerCamel member path of the model
/// ([`::aws_smithy_schema_server::constraints::path`]) and stating the one requirement it
/// broke, so tooling that reads the failing member out of a validation error finds it here
/// too. The envelope and the clause are the framework's own rendering of an absent member,
/// so this sentence cannot drift from the one a member the model DOES require is refused
/// with; the final stop is how Lambda ends it.
fn absent_required_member(member: &str) -> String {
    let path = ::aws_smithy_schema_server::constraints::path::member(member);
    format!(
        "{}.",
        ::aws_smithy_schema_server::missing_member_message(&path)
    )
}

/// [`absent_required_member`] for a member the request carries INSIDE a structure, whose
/// path is the structure's member path and then its own — the same spelling the constraint
/// walk produces for a violation found inside a structure.
fn absent_required_nested_member(structure: &str, member: &str) -> String {
    let path = format!(
        "{}.{}",
        ::aws_smithy_schema_server::constraints::path::member(structure),
        ::aws_smithy_schema_server::constraints::path::member(member)
    );
    format!(
        "{}.",
        ::aws_smithy_schema_server::missing_member_message(&path)
    )
}

/// How a request whose `EventSourceArn` names nothing an event source mapping can poll is
/// refused.
///
/// Both refusals open with the same sentence — the source was not recognized — and then
/// state the ONE thing that distinguishes the two cases, which is what tooling matches on.
///
/// An ARN whose VENDOR is one of the source services that member enumerates DOES name a
/// service a mapping polls, so what the value got wrong is the SHAPE of the resource: the
/// refusal states the ARN format that service's pollable resource is addressed by
/// ([`expected_event_source_resource`]), because the missing or extra segment is the only
/// thing that tells the caller what to send instead. An ARN of any other vendor names no
/// source service at all, so there is no format to expect and nothing about the resource
/// half to state: the refusal says the source ARN itself is unsupported and quotes it, which
/// is the only variable part of that sentence.
///
/// What that format states is the SHAPE of the ARN rather than any one resource: the
/// resource half is written with the placeholder names the format uses
/// ([`expected_event_source_resource`]) and the account half with the example account id
/// ([`EXAMPLE_ACCOUNT_ID`]), because neither names a resource of anybody's. The ONE field of
/// it that is the caller's own is the Region, which the resources of a mapping's event source
/// live in, so the example is written for the Region this request was made in.
fn unrecognized_event_source(vendor: &str, event_source: &str, region: &str) -> String {
    match expected_event_source_resource(vendor) {
        Some(resource) => format!(
            "Unrecognized event source. Expected arn of format \
             arn:aws:{vendor}:{region}:{EXAMPLE_ACCOUNT_ID}:{resource}"
        ),
        None => format!("Unrecognized event source. Unsupported source arn : {event_source}"),
    }
}

/// The RESOURCE field of the ARN that addresses the resource one source service is polled
/// through, for each vendor namespace `EventSourceArn` enumerates — and `None` for a vendor
/// that enumeration does not name.
///
/// One row per entry of that member's list, each written as that service writes the ARN of
/// the resource the entry names: "Amazon Kinesis – The ARN of the data stream or a stream
/// consumer", "Amazon DynamoDB Streams – The ARN of the stream" (the stream OF a table,
/// hence the table's own resource field plus the stream segment), "Amazon Simple Queue
/// Service – The ARN of the queue" (whose resource field is the queue's name), "Amazon
/// Managed Streaming for Apache Kafka – The ARN of the cluster" (the VPC connection the
/// same entry documents is its cross-account form rather than the shape a same-account
/// mapping names), "Amazon MQ – The ARN of the broker" and "Amazon DocumentDB – The ARN of
/// the DocumentDB change stream", which is an Amazon RDS cluster.
///
/// The placeholder names are the ones the format states for a shape rather than values of
/// any resource, so the sentence reads the same whatever the caller sent.
fn expected_event_source_resource(vendor: &str) -> Option<&'static str> {
    Some(match vendor {
        crate::state::EVENT_SOURCE_VENDOR_KINESIS => "stream/StreamName",
        crate::state::EVENT_SOURCE_VENDOR_DYNAMODB => "table/TableName/stream/StreamId",
        crate::state::EVENT_SOURCE_VENDOR_QUEUE => "QueueName",
        crate::state::EVENT_SOURCE_VENDOR_KAFKA => "cluster/ClusterName/ClusterId",
        crate::state::EVENT_SOURCE_VENDOR_MQ => "broker:BrokerName:BrokerId",
        crate::state::EVENT_SOURCE_VENDOR_DOCUMENT_DB => "cluster:ClusterName",
        _ => return None,
    })
}

/// The modelled 400 this operation refuses a request it cannot satisfy with.
fn invalid_parameter(message: String) -> CreateEventSourceMappingError {
    CreateEventSourceMappingErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            r#type: None,
            message: Some(message),
        },
    )
    .into()
}
