// SPDX-License-Identifier: CC0-1.0

//! @op UpdateEventSourceMapping
//!
//! Updates the ONE event source mapping the `UUID` names and answers the
//! `EventSourceMappingConfiguration` read back out of what it stored.
//!
//! Every member the request carries replaces the stored value and every member it omits
//! leaves the stored value alone, so a call that states only `Enabled` pauses or resumes
//! the mapping without touching its batching, filtering or destination settings. A
//! `FunctionName` re-targets the mapping onto the function (and, when the name qualifies
//! one, the version or alias) it names, which has to resolve to a function this emulator
//! holds.
//!
//! A re-target states a new pair of event source and function, so the pair is held to the
//! same rule the create of a mapping is: a QUEUE whose `VisibilityTimeout` is below the
//! newly named function's `Timeout` would make a message visible to a second poller while
//! the function is still working on it, and that pair is refused with the modelled
//! `InvalidParameterValueException` (see [`crate::state::queue_visibility_refusal`]). The
//! same modelled refusal covers the other rule the pair is held to: Lambda reads the source
//! acting as the invoked function's execution role, so a re-target onto a function whose
//! role may not read the source this mapping polls changes nothing (see
//! [`crate::state::event_source_authorization`]).
//!
//! A `MetricsConfig` the update states is held to the same rule the create's is: it is a SET
//! of the values `EventSourceMappingMetricsConfig$Metrics` declares drawn from the ones the
//! mapping's own event source supports, so a value that source cannot produce is refused with
//! the modelled `InvalidParameterValueException` naming the values it does expect, and a
//! selection that repeats a value is refused with that same modelled error carrying its own
//! fixed sentence; in either case nothing changes (see
//! [`crate::state::metrics_config_refusal`]).
//!
//! A `DestinationConfig` the update states is held to the same rule the create's is too:
//! each destination it names has to be one of the resources Lambda writes a record of a
//! failed batch to — the kinds `OnFailure$Destination` enumerates — and one of any other
//! service is refused with that same modelled 400 and changes nothing (see
//! [`crate::state::unsupported_mapping_destination`]); and the write that record takes has to
//! be one the EXECUTION ROLE of the function the mapping invokes after this update may make,
//! since that is the identity it is written under (see
//! [`crate::state::unwritable_mapping_destination`]). A `kafka://<topic>` destination, the one
//! kind of that enumeration that is no ARN, takes PROVISIONED MODE — the mode in force after
//! the update — and may not name a topic the mapping consumes, and is refused with that same
//! modelled 400 otherwise (see [`crate::state::unsupported_kafka_topic_destination`]). A
//! `SchemaRegistryConfig` it states takes that same mode, since the records are validated
//! against the registry by those same pollers (see
//! [`crate::state::schema_registry_config_refusal`]).
//!
//! So are the two settings a request about WHAT the mapping polls states: `FilterCriteria`
//! carrying text that is not a filter pattern decide nothing, so they are refused with that
//! same modelled 400 and the criteria the mapping already carries stand (see
//! [`crate::state::filter_criteria_refusal`]); and `BatchSize`'s own related setting — "When
//! you set BatchSize to a value greater than 10, you must set
//! MaximumBatchingWindowInSeconds to at least 1" — is decided against the values in force
//! after this update, so a mapping on a queue is never left with a batch it has no window to
//! gather (see [`crate::state::queue_batch_window_refusal`]). And the `KMSKeyArn` in force
//! after the update is the customer managed key the criteria in force after it are sealed
//! under, which Lambda opens again as its own service principal: a key whose policy withholds
//! that use is refused with that same modelled 400 and the mapping keeps the key and criteria
//! it had (see [`crate::state::filter_criteria_key_denial`]).
//!
//! A setting whose own documentation gives an EMPTY value a meaning is read that way rather
//! than stored as it arrived: "To delete the filter criteria, specify an empty FilterCriteria
//! object" and "(Amazon SQS only) The scaling configuration for the event source. To remove
//! the configuration, pass an empty value" (`ScalingConfig`) — so a `FilterCriteria` carrying
//! no filter and a `ScalingConfig` limiting nothing each REMOVE what the mapping carried, and
//! every reader of it then reports the member not at all (see
//! [`crate::state::criteria_carry_no_filter`] and
//! [`crate::state::scaling_config_sets_no_limit`]).
//!
//! The operation answers 202 Accepted, so the change is NOT in effect when the call
//! returns: the mapping moves into the transitional state of the change it accepted — one
//! of the states `EventSourceMappingConfiguration$State` enumerates — and the move to the
//! state its `Enabled` setting asks for is owed by the CLOCK (see
//! [`crate::state::EventSourceMappingRecord::begin_update`]). Nothing sleeps for it.
//! WHICH of them it is follows from what the update changes, because this is the call that
//! can "pause invocation and resume later": an update that turns polling off reports
//! `Disabling` until the pause is in force, one that turns it back on reports `Enabling`,
//! and one that leaves polling exactly as it found it changed the CONFIGURATION and reports
//! `Updating` — so a client that paused a consumer and is waiting for the pause to take hold
//! reads the pause's own window rather than a state that names an unrelated change.
//!
//! The window a mapping is UNAVAILABLE to an update in is the one its own CREATE opens:
//! `ResourceInUseException` is "The operation conflicts with the resource's availability.
//! For example, you tried to update an event source mapping in the CREATING state, or you
//! tried to delete an event source mapping currently UPDATING" — so an update that arrives
//! while the mapping is still being stood up is refused with that modelled error (see
//! [`crate::state::EventSourceMappingRecord::is_creating`]), while the window an accepted
//! change is being applied in is the one a DELETE is refused inside. A mapping that has
//! settled out of its create therefore takes every further update, one after another and
//! without waiting for the one before it to settle, and the last change accepted is the one
//! that stands — which is what makes a setting reconfigurable and clearable at all. A value
//! a member states is decided against its own constraints whatever the mapping is doing.
//!
//! A mapping whose DELETION has already been accepted is a different case: it is not an
//! unavailable mapping but one on its way out, with nothing left to change, so an update of
//! it answers the modelled not-found (HTTP 404) that an unheld `UUID` gets — the windows the
//! sentence above names are the ones a mapping is still THERE for. A READ of it still
//! reports the `Deleting` state the teardown is in, because "it enters a Deleting state and
//! might not be completely deleted for several seconds" (`DeleteEventSourceMapping`).
//!
//! An update is also what RESUMES polling, or re-targets it onto another queue, so it arms
//! the mapping's own poller exactly as the create does
//! ([`crate::state::drive_sqs_pollers`], [`crate::state::drive_mq_pollers`],
//! [`crate::state::drive_kafka_pollers`]); a poller reads only while its mapping reports
//! `Enabled` at the instant of each pass, so a mapping this call paused stops being polled
//! when the clock settles it into `Disabled`, and its poller stands down — and, for a mapping
//! on an Amazon MSK cluster, leaves the consumer group it was reading as, keeping the offsets
//! it committed so the resumed poller reads on from them.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateEventSourceMapping::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `UpdateEventSourceMapping`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")
//! ```
//!
//! Updates an event source mapping. You can change the function that Lambda invokes, or
//! pause invocation and resume later from the same location. For details about how to
//! configure different event sources, see the following topics. Amazon DynamoDB Streams
//! Amazon Kinesis Amazon SQS Amazon MQ and RabbitMQ Amazon MSK Apache Kafka Amazon
//! DocumentDB The following error handling options are available for stream sources
//! (DynamoDB, Kinesis, Amazon MSK, and self-managed Apache Kafka):
//! BisectBatchOnFunctionError – If the function returns an error, split the batch in two
//! and retry. MaximumRecordAgeInSeconds – Discard records older than the specified age. The
//! default value is infinite (-1). When set to infinite (-1), failed records are retried
//! until the record expires MaximumRetryAttempts – Discard records after the specified
//! number of retries. The default value is infinite (-1). When set to infinite (-1), failed
//! records are retried until the record expires. OnFailure – Send discarded records to an
//! Amazon SQS queue, Amazon SNS topic, Kafka topic, or Amazon S3 bucket. For more
//! information, see Adding a destination. The following option is available only for
//! DynamoDB and Kinesis event sources: ParallelizationFactor – Process multiple batches
//! from each shard concurrently. For information about which configuration parameters apply
//! to each event source, see the following topics. Amazon DynamoDB Streams Amazon Kinesis
//! Amazon SQS Amazon MQ and RabbitMQ Amazon MSK Apache Kafka Amazon DocumentDB
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
//! * `FilterCriteria` (FilterCriteria) — An object that defines the filter criteria that
//!   determine whether Lambda should process an event. For more information, see Lambda event
//!   filtering.
//! * `FunctionName` (NamespacedFunctionName, len 1..256, pattern
//!   ^(arn:(aws[a-zA-Z-]*)?:lambda:)?((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:)?(\d{12}:)?(function:)?([a-zA-Z0
//!   …) — The name or ARN of the Lambda function. Name formats Function name – MyFunction.
//!   Function ARN – arn:aws:lambda:us-west-2:123456789012:function:MyFunction. Version or
//!   Alias ARN – arn:aws:lambda:us-west-2:123456789012:function:MyFunction:PROD. Partial ARN
//!   – 123456789012:function:MyFunction. The length constraint applies only to the full ARN.
//!   If you specify only the function name, it's limited to 64 characters in length.
//! * `FunctionResponseTypes` (FunctionResponseTypeList, len 0..1) — (Kinesis, DynamoDB
//!   Streams, Amazon MSK, self-managed Apache Kafka, and Amazon SQS) A list of current
//!   response type enums applied to the event source mapping.
//! * `KMSKeyArn` (KMSKeyArn, len 0..10000, pattern
//!   ^(arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*)|()$) — The ARN of the Key Management Service
//!   (KMS) customer managed key that Lambda uses to encrypt your function's filter criteria.
//!   By default, Lambda does not encrypt your filter criteria object. Specify this property
//!   to encrypt data using your own customer managed key.
//! * `LoggingConfig` (EventSourceMappingLoggingConfig) — (Amazon MSK, and self-managed Apache
//!   Kafka only) The logging configuration for your event source. Use this configuration
//!   object to define the level of logs for your event source mapping.
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
//! * `ProvisionedPollerConfig` (ProvisionedPollerConfig) — (Amazon SQS, Amazon MSK, and
//!   self-managed Apache Kafka only) The provisioned mode configuration for the event source.
//!   For more information, see provisioned mode.
//! * `ScalingConfig` (ScalingConfig) — (Amazon SQS only) The scaling configuration for the
//!   event source. For more information, see Configuring maximum concurrency for Amazon SQS
//!   event sources.
//! * `UUID` (UUIDString, len 36..36) **required** — The identifier of the event source
//!   mapping.
//! * +3 more omitted for length; get all of them with:
//!   smithy_query(query="shape",shape="com.amazonaws.lambda#UpdateEventSourceMappingRequest",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")
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
//! * `ResourceInUseException` (HTTP 400) — The operation conflicts with the resource's
//!   availability. For example, you tried to update an event source mapping in the CREATING
//!   state, or you tried to delete an event source mapping currently UPDATING.
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
// `UpdateEventSourceMappingError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateEventSourceMappingError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.lambda#UpdateEventSourceMapping")]
#[derive(Debug)]
pub enum UpdateEventSourceMappingErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    ResourceConflictException(ResourceConflictException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceException(ServiceException),
    TooManyRequestsException(TooManyRequestsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_event_source_mapping::UpdateEventSourceMappingError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateEventSourceMappingError = crate::scaffolded::Scaffolded<UpdateEventSourceMappingErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateEventSourceMappingErrorModeled> for UpdateEventSourceMappingError {
    fn from(modeled: UpdateEventSourceMappingErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateEventSourceMapping` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateEventSourceMapping<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateEventSourceMapping<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateEventSourceMapping<Eng>
{
    type Input = UpdateEventSourceMappingInput;
    type Output = UpdateEventSourceMappingOutput;
    type Error = UpdateEventSourceMappingError;

    const NAME: &'static str = "UpdateEventSourceMapping";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_EVENT_SOURCE_MAPPING_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_EVENT_SOURCE_MAPPING_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(service_error("Lambda state is not reachable on this engine"));
        };

        // A member whose `@pattern` ALTERNATES whole-value branches is satisfied only by a
        // value one branch matches end to end, which the boundary walk cannot judge for an
        // expression anchored at its two ends alone — so the member carrying this model's "an
        // ARN, or nothing at all" expression is judged here (see
        // [`crate::state::arn_or_empty_refusal`]), on the REQUEST and before the record is
        // read or touched.
        if let Some(message) =
            crate::state::arn_or_empty_refusal(&[(&["KMSKeyArn"], input.kms_key_arn.as_deref())])
        {
            return Err(crate::scaffolded::Scaffolded::validation(message));
        }

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);
        let mut store = state.lock();
        // The store is first brought up to whatever the clock already made due, so a
        // mapping whose own removal was already owed is gone before this call looks for
        // it, and a window the clock has already closed is not read as still open.
        store.materialize_all(now);

        {
            let Some(record) = store.event_source_mappings.get(&input.uuid) else {
                // A mapping is addressed by its own identifier, which the request already
                // carries, so the refusal is the fixed sentence rather than the resource
                // named back — the same sentence every other operation on a mapping
                // refuses an unheld `UUID` with.
                return Err(not_found(
                    crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
                ));
            };
            // A mapping whose deletion this account has ALREADY accepted is on its way out:
            // there is nothing left to change, so the UUID answers the same modelled
            // not-found an unheld one does, even while a READ still reports the `Deleting`
            // state the teardown is in. See the module docs.
            if record.is_deleting() {
                return Err(not_found(
                    crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
                ));
            }
            // "The operation conflicts with the resource's availability": the window a
            // mapping is not available to be UPDATED in is the one its own create opens.
            // See the module docs.
            if record.is_creating() {
                // The refusal is a FIXED sentence: it states that the mapping is in use,
                // and names neither the mapping nor the state it is settling through.
                return Err(resource_in_use(
                    "Cannot update the event source mapping because it is in use.".to_string(),
                ));
            }
        }

        // The event source this mapping polls, which no update changes: a `FunctionName`
        // picks a new function for the SAME source, so it is the source the new function is
        // checked against below.
        let mapped_source = store
            .event_source_mappings
            .get(&input.uuid)
            .and_then(|record| record.event_source_arn.clone());
        // A mapping names exactly one event source, so one with no ARN is the self-managed
        // Apache Kafka cluster the create named instead.
        let self_managed = mapped_source.is_none();
        // A `MetricsConfig` this update states is held to exactly what the create's is: a
        // SET of the values the enum declares drawn from the ones the source the mapping
        // polls supports (see [`crate::state::metrics_config_refusal`]) — a repeated value
        // and a value that source cannot produce each carry their own sentence. A selection
        // it cannot satisfy changes nothing.
        if let Some(message) = crate::state::metrics_config_refusal(
            input.metrics_config.as_ref(),
            mapped_source
                .as_deref()
                .and_then(crate::state::event_source_vendor),
            self_managed,
        ) {
            return Err(invalid_parameter(message));
        }

        // A `DestinationConfig` this update states is held to exactly what the create's is:
        // every destination it names has to be one of the resources Lambda writes a record
        // of a failed batch to (see [`crate::state::unsupported_mapping_destination`]). One
        // of any other service changes nothing.
        if let Some(message) =
            crate::state::unsupported_mapping_destination(input.destination_config.as_ref())
        {
            return Err(invalid_parameter(message));
        }

        // And a Kafka-topic destination it states is held to the same rules as a create's: it
        // is a topic of the mapping's own cluster, produced to by the mapping's own pollers,
        // so it takes PROVISIONED MODE and may not be a topic the mapping consumes (see
        // [`crate::state::unsupported_kafka_topic_destination`]). Both are judged against the
        // mapping AFTER this update: the mode this request states, or the one the mapping
        // already holds when it states none, and the topics, which no update changes.
        if let Some(message) = crate::state::unsupported_kafka_topic_destination(
            input.destination_config.as_ref(),
            input.provisioned_poller_config.as_ref().or_else(|| {
                store
                    .event_source_mappings
                    .get(&input.uuid)
                    .and_then(|record| record.provisioned_poller_config.as_ref())
            }),
            store
                .event_source_mappings
                .get(&input.uuid)
                .and_then(|record| record.topics.as_deref()),
        ) {
            return Err(invalid_parameter(message));
        }

        // A `SchemaRegistryConfig` this update states is held to exactly what the create's is:
        // the records are validated against it by the mapping's own pollers, so it takes the
        // pollers PROVISIONED MODE provides (see
        // [`crate::state::schema_registry_config_refusal`]) — the mode in force AFTER this
        // update, which is what this request states or what the mapping already holds. One
        // stated outside that mode changes nothing.
        if let Some(message) = crate::state::schema_registry_config_refusal(
            input.self_managed_kafka_event_source_config.as_ref(),
            input.amazon_managed_kafka_event_source_config.as_ref(),
            input.provisioned_poller_config.as_ref().or_else(|| {
                store
                    .event_source_mappings
                    .get(&input.uuid)
                    .and_then(|record| record.provisioned_poller_config.as_ref())
            }),
        ) {
            return Err(invalid_parameter(message));
        }

        // And `FilterCriteria` this update states are held to exactly what the create's are:
        // each `Filter$Pattern` is "a filter pattern", so text that is not one decides
        // nothing and is refused rather than stored over criteria that did (see
        // [`crate::state::filter_criteria_refusal`]) — which is also what keeps a later,
        // valid update of the same mapping possible.
        if let Some(message) = crate::state::filter_criteria_refusal(input.filter_criteria.as_ref())
        {
            return Err(invalid_parameter(message.to_string()));
        }

        // And the KEY those criteria are sealed under is held to exactly what the create's is:
        // Lambda opens an encrypted mapping's criteria under its own service principal, which
        // holds no identity-based policies, so a key whose own policy withholds that use is
        // refused rather than stored over the key the mapping had (see
        // [`crate::state::filter_criteria_key_denial`]). Both halves are the values in force
        // AFTER this update — what this request states for each, and the stored value for the
        // half it leaves alone — because that pair is what the criteria would be sealed under.
        if let Some(message) = store
            .event_source_mappings
            .get(&input.uuid)
            .and_then(|record| {
                crate::state::filter_criteria_key_denial(
                    &*self.engine,
                    &region,
                    input
                        .kms_key_arn
                        .as_deref()
                        .or(record.kms_key_arn.as_deref()),
                    input
                        .filter_criteria
                        .as_ref()
                        .or(record.filter_criteria.as_ref()),
                    &record.arn,
                )
            })
        {
            return Err(invalid_parameter(message.to_string()));
        }

        // And the pair `BatchSize`'s own related setting binds is held to the same rule a
        // create is, against the values in force AFTER this update: what this request states
        // for each half, and the stored value for the half it leaves alone (see
        // [`crate::state::queue_batch_window_refusal`]).
        if let Some(message) = store
            .event_source_mappings
            .get(&input.uuid)
            .and_then(|record| {
                crate::state::queue_batch_window_refusal(
                    mapped_source.as_deref(),
                    input.batch_size.unwrap_or(record.batch_size),
                    input
                        .maximum_batching_window_in_seconds
                        .or(record.maximum_batching_window_in_seconds),
                )
            })
        {
            return Err(invalid_parameter(message.to_string()));
        }

        // Only a queue carries a visibility timeout, so only a queue source is one of those
        // comparisons.
        let queue_source = mapped_source.clone().filter(|arn| {
            crate::state::event_source_vendor(arn) == Some(crate::state::EVENT_SOURCE_VENDOR_QUEUE)
        });

        // A `FunctionName` re-targets the mapping, so it has to resolve to a function
        // this store holds — in any of its documented forms, qualified or not.
        let (retarget, retarget_timeout, retarget_role) = match input.function_name.as_deref() {
            None => (None, None, None),
            Some(function_name) => {
                let (function, version) =
                    crate::state::resolve_function(&mut store, function_name, None, &region, now)
                        .map_err(|failure| match failure {
                            crate::state::ResolveFailure::Malformed(message) => {
                                invalid_parameter(message)
                            }
                            crate::state::ResolveFailure::NotFound(message) => not_found(message),
                        })?;
                // "The amount of time in seconds that Lambda allows a function to run
                // before stopping it", for the version this mapping would invoke. The
                // resolution above refuses a version this store holds no configuration
                // for, so a version with no timeout to compare is a failure of the store
                // rather than of the request. The same configuration carries the EXECUTION
                // ROLE the mapping's poller would read the event source as.
                let function_arn = function.versioned_arn(&version);
                let Some((timeout, role)) = function
                    .config_of(&version)
                    .map(|config| (config.timeout, config.role.clone()))
                else {
                    return Err(service_error(&format!(
                        "Lambda resolved {function_arn} without a configuration to read its \
                         timeout from"
                    )));
                };
                (
                    Some((
                        function.config.name.clone(),
                        function_arn,
                        // `$LATEST` is the function itself rather than one version or
                        // alias of it, so a reference to it is an unqualified reference.
                        (version != crate::state::VERSION_LATEST).then_some(version),
                    )),
                    Some(timeout),
                    Some(role),
                )
            }
        };

        // Re-targeting states a new pair of queue and function, so the pair is held to the
        // same rule a create is: a queue whose visibility timeout is below the function's
        // own would make a message visible to a second poller mid-invocation.
        if let Some(message) = queue_source
            .as_deref()
            .zip(retarget_timeout)
            .and_then(|(queue, function_timeout)| {
                crate::state::queue_visibility_timeout_seconds(&*self.engine, queue).and_then(
                    |visibility_timeout| {
                        crate::state::queue_visibility_refusal(visibility_timeout, function_timeout)
                    },
                )
            })
        {
            return Err(invalid_parameter(message));
        }

        // And to the other rule a create is held to: Lambda reads the source acting as the
        // invoked function's execution role, so a re-target onto a function whose role may
        // not read the source this mapping polls is refused and changes nothing (see
        // [`crate::state::event_source_authorization`]).
        if let Some(message) = mapped_source
            .as_deref()
            .zip(retarget_role.as_deref())
            .and_then(|(event_source, role)| {
                crate::state::unreadable_event_source(&*self.engine, role, event_source)
            })
        {
            return Err(invalid_parameter(message));
        }

        // And to the third: Lambda WRITES the record of a batch the mapping gives up on as
        // that same role, so a `DestinationConfig` this update states is held to what the
        // role in force after it may write (see
        // [`crate::state::unwritable_mapping_destination`]). The role in force is the
        // re-targeted function's where the request names one, and the mapping's own
        // otherwise — which is why a destination stated beside no `FunctionName` is still
        // decided.
        let destination_role = retarget_role.clone().or_else(|| {
            let mapping = store.event_source_mappings.get(&input.uuid)?;
            let function = store.functions.get(&mapping.function_name)?;
            let version = mapping
                .function_qualifier
                .as_deref()
                .and_then(|qualifier| function.resolved_version(qualifier))
                .unwrap_or_else(|| crate::state::VERSION_LATEST.to_string());
            function.config_of(&version).map(|config| config.role.clone())
        });
        if let Some(message) = destination_role.as_deref().and_then(|role| {
            crate::state::unwritable_mapping_destination(
                &*self.engine,
                role,
                input.destination_config.as_ref(),
            )
        }) {
            return Err(invalid_parameter(message));
        }

        let Some(record) = store.event_source_mappings.get_mut(&input.uuid) else {
            return Err(service_error(
                "Lambda lost the event source mapping between resolving it and updating it",
            ));
        };

        // Whether this mapping was POLLING when the update arrived, read before any member
        // is replaced: which transitional state the accepted change reports follows from
        // whether the change turns polling on, off, or leaves it where it was (see
        // [`crate::state::EventSourceMappingRecord::begin_update`]).
        let polled_before = record.enabled;

        if let Some((function_name, function_arn, function_qualifier)) = retarget {
            record.function_name = function_name;
            record.function_arn = function_arn;
            record.function_qualifier = function_qualifier;
        }
        // Every member the request states replaces the stored value; every member it
        // omits leaves the stored value alone.
        if let Some(enabled) = input.enabled {
            record.enabled = enabled;
        }
        if let Some(batch_size) = input.batch_size {
            record.batch_size = batch_size;
        }
        // `FilterCriteria` the request states replace the stored criteria — and criteria
        // that carry no filter at all are what DELETES them ("To delete the filter criteria,
        // specify an empty FilterCriteria object"), so the mapping is left with no criteria
        // rather than with an empty structure, and every reader of it reports the member not
        // at all.
        if let Some(criteria) = input.filter_criteria.clone() {
            record.filter_criteria =
                Some(criteria).filter(|stated| !crate::state::criteria_carry_no_filter(stated));
        }
        if input.kms_key_arn.is_some() {
            record.kms_key_arn = input.kms_key_arn.clone();
        }
        if input.metrics_config.is_some() {
            record.metrics_config = input.metrics_config.clone();
        }
        if input.logging_config.is_some() {
            record.logging_config = input.logging_config.clone();
        }
        if let Some(scaling_config) = input.scaling_config.clone() {
            // A scaling configuration the request states replaces the stored one — and one
            // that limits nothing is what REMOVES it ("To remove the configuration, pass an
            // empty value"), so the mapping is left with no scaling configuration rather
            // than with a structure that limits nothing, and every reader of it reports the
            // member not at all.
            record.scaling_config = Some(scaling_config)
                .filter(|stated| !crate::state::scaling_config_sets_no_limit(stated));
        }
        if input.maximum_batching_window_in_seconds.is_some() {
            record.maximum_batching_window_in_seconds = input.maximum_batching_window_in_seconds;
        }
        if input.parallelization_factor.is_some() {
            record.parallelization_factor = input.parallelization_factor;
        }
        if input.destination_config.is_some() {
            record.destination_config = input.destination_config.clone();
        }
        if input.maximum_record_age_in_seconds.is_some() {
            record.maximum_record_age_in_seconds = input.maximum_record_age_in_seconds;
        }
        if input.bisect_batch_on_function_error.is_some() {
            record.bisect_batch_on_function_error = input.bisect_batch_on_function_error;
        }
        if input.maximum_retry_attempts.is_some() {
            record.maximum_retry_attempts = input.maximum_retry_attempts;
        }
        if input.tumbling_window_in_seconds.is_some() {
            record.tumbling_window_in_seconds = input.tumbling_window_in_seconds;
        }
        if input.source_access_configurations.is_some() {
            record.source_access_configurations = input.source_access_configurations.clone();
        }
        if let Some(function_response_types) = input.function_response_types.clone() {
            record.function_response_types = function_response_types;
        }
        if input.amazon_managed_kafka_event_source_config.is_some() {
            record.amazon_managed_kafka_event_source_config =
                input.amazon_managed_kafka_event_source_config.clone();
        }
        if input.self_managed_kafka_event_source_config.is_some() {
            record.self_managed_kafka_event_source_config =
                input.self_managed_kafka_event_source_config.clone();
        }
        if input.document_db_event_source_config.is_some() {
            record.document_db_event_source_config = input.document_db_event_source_config.clone();
        }
        if input.provisioned_poller_config.is_some() {
            record.provisioned_poller_config = input.provisioned_poller_config.clone();
        }

        // The change is accepted rather than applied: the mapping is in the transitional
        // state of the change it accepted — `Enabling` for a resume, `Disabling` for a
        // pause, `Updating` for a change that leaves polling where it was — until the clock
        // makes the move out of it due. See the module docs.
        record.begin_update(now, polled_before);

        // Answered by reading the stored record back, so `GetEventSourceMapping` reports
        // exactly what this call persisted.
        let reported = event_source_mapping_configuration(record.reported());

        // An update may be what ENABLES a mapping, or what re-targets it onto another queue
        // or function, so it arms the pollers exactly as the create does, through the one call
        // that arms them ([`crate::state::arm_mapping_pollers`]): each reader decides for
        // itself whether it has anything to do, and a mapping that already had a reader keeps
        // the one it has (the claim keeps a second from starting). The store's lock is
        // released first, because each poller takes it itself.
        drop(store);
        crate::state::arm_mapping_pollers(ctx, &self.engine);

        Ok(reported)
    }
}

/// The one `EventSourceMappingConfiguration` the model declares, in this operation's own
/// generated shape of it.
///
/// [`crate::state::EventSourceMappingRecord::reported`] builds the shared one (generated
/// as `CreateEventSourceMappingOutput`) that every operation reporting a mapping answers
/// with, so the members are carried across one for one rather than rebuilt from state a
/// second time.
fn event_source_mapping_configuration(
    configuration: CreateEventSourceMappingOutput,
) -> UpdateEventSourceMappingOutput {
    UpdateEventSourceMappingOutput {
        uuid: configuration.uuid,
        starting_position: configuration.starting_position,
        starting_position_timestamp: configuration.starting_position_timestamp,
        batch_size: configuration.batch_size,
        maximum_batching_window_in_seconds: configuration.maximum_batching_window_in_seconds,
        parallelization_factor: configuration.parallelization_factor,
        event_source_arn: configuration.event_source_arn,
        filter_criteria: configuration.filter_criteria,
        filter_criteria_error: configuration.filter_criteria_error,
        kms_key_arn: configuration.kms_key_arn,
        metrics_config: configuration.metrics_config,
        logging_config: configuration.logging_config,
        scaling_config: configuration.scaling_config,
        function_arn: configuration.function_arn,
        last_modified: configuration.last_modified,
        last_processing_result: configuration.last_processing_result,
        state: configuration.state,
        state_transition_reason: configuration.state_transition_reason,
        destination_config: configuration.destination_config,
        topics: configuration.topics,
        queues: configuration.queues,
        source_access_configurations: configuration.source_access_configurations,
        self_managed_event_source: configuration.self_managed_event_source,
        maximum_record_age_in_seconds: configuration.maximum_record_age_in_seconds,
        bisect_batch_on_function_error: configuration.bisect_batch_on_function_error,
        maximum_retry_attempts: configuration.maximum_retry_attempts,
        tumbling_window_in_seconds: configuration.tumbling_window_in_seconds,
        function_response_types: configuration.function_response_types,
        amazon_managed_kafka_event_source_config: configuration
            .amazon_managed_kafka_event_source_config,
        self_managed_kafka_event_source_config: configuration
            .self_managed_kafka_event_source_config,
        document_db_event_source_config: configuration.document_db_event_source_config,
        event_source_mapping_arn: configuration.event_source_mapping_arn,
        provisioned_poller_config: configuration.provisioned_poller_config,
    }
}

/// The modelled 400 for a `FunctionName` that is not one of the documented name forms, and
/// for a re-target whose queue would re-deliver a message mid-invocation.
fn invalid_parameter(message: String) -> UpdateEventSourceMappingError {
    UpdateEventSourceMappingErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            r#type: None,
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 404 for a `UUID` that names no mapping, or a `FunctionName` that names no
/// function this emulator holds.
fn not_found(message: String) -> UpdateEventSourceMappingError {
    UpdateEventSourceMappingErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        r#type: None,
        message: Some(message),
    })
    .into()
}

/// The modelled refusal for a mapping that is not available to take this change.
fn resource_in_use(message: String) -> UpdateEventSourceMappingError {
    UpdateEventSourceMappingErrorModeled::ResourceInUseException(ResourceInUseException {
        r#type: None,
        message: Some(message),
    })
    .into()
}

/// The modelled 500 for a failure of the store itself.
fn service_error(message: &str) -> UpdateEventSourceMappingError {
    UpdateEventSourceMappingErrorModeled::ServiceException(ServiceException {
        r#type: None,
        message: Some(message.to_string()),
    })
    .into()
}
