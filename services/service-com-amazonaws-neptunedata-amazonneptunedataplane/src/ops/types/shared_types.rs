// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached by more than one operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
//!
//! HAND-WRITTEN at a generated path, and so NOT re-rendered by a model refresh, for one
//! reason: every exception this service declares is written to the wire with one member
//! more than the on-disk model lists — the `message` an SDK client surfaces as the error's
//! message — and `#[smithy_macros::smithy(shape = ...)]` admits exactly the model's member
//! list, so a member the service really sends cannot be declared through that derive at
//! all. Those shapes are declared here with `neptune_error_shape!` instead, which emits
//! the same items for the member list `types::error_message_wire` states; every OTHER
//! shape below is still declared from the model by the derive.
//!
//! A model refresh that adds, removes or restates a shape reached by more than one
//! operation therefore has to be followed here by hand: an exception as one more
//! `neptune_error_shape!`, anything else as one more derive.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-neptunedata-amazonneptunedataplane.json");

neptune_error_shape!(
    BadRequestException,
    SCHEMA_BAD_REQUEST_EXCEPTION,
    "com.amazonaws.neptunedata#BadRequestException",
    "com.amazonaws.neptunedata",
    "BadRequestException",
);

neptune_error_shape!(
    ClientTimeoutException,
    SCHEMA_CLIENT_TIMEOUT_EXCEPTION,
    "com.amazonaws.neptunedata#ClientTimeoutException",
    "com.amazonaws.neptunedata",
    "ClientTimeoutException",
);

neptune_error_shape!(
    ConcurrentModificationException,
    SCHEMA_CONCURRENT_MODIFICATION_EXCEPTION,
    "com.amazonaws.neptunedata#ConcurrentModificationException",
    "com.amazonaws.neptunedata",
    "ConcurrentModificationException",
);

neptune_error_shape!(
    ConstraintViolationException,
    SCHEMA_CONSTRAINT_VIOLATION_EXCEPTION,
    "com.amazonaws.neptunedata#ConstraintViolationException",
    "com.amazonaws.neptunedata",
    "ConstraintViolationException",
);

neptune_error_shape!(
    FailureByQueryException,
    SCHEMA_FAILURE_BY_QUERY_EXCEPTION,
    "com.amazonaws.neptunedata#FailureByQueryException",
    "com.amazonaws.neptunedata",
    "FailureByQueryException",
);

neptune_error_shape!(
    IllegalArgumentException,
    SCHEMA_ILLEGAL_ARGUMENT_EXCEPTION,
    "com.amazonaws.neptunedata#IllegalArgumentException",
    "com.amazonaws.neptunedata",
    "IllegalArgumentException",
);

neptune_error_shape!(
    InvalidArgumentException,
    SCHEMA_INVALID_ARGUMENT_EXCEPTION,
    "com.amazonaws.neptunedata#InvalidArgumentException",
    "com.amazonaws.neptunedata",
    "InvalidArgumentException",
);

neptune_error_shape!(
    InvalidParameterException,
    SCHEMA_INVALID_PARAMETER_EXCEPTION,
    "com.amazonaws.neptunedata#InvalidParameterException",
    "com.amazonaws.neptunedata",
    "InvalidParameterException",
);

neptune_error_shape!(
    MissingParameterException,
    SCHEMA_MISSING_PARAMETER_EXCEPTION,
    "com.amazonaws.neptunedata#MissingParameterException",
    "com.amazonaws.neptunedata",
    "MissingParameterException",
);

neptune_error_shape!(
    ParsingException,
    SCHEMA_PARSING_EXCEPTION,
    "com.amazonaws.neptunedata#ParsingException",
    "com.amazonaws.neptunedata",
    "ParsingException",
);

neptune_error_shape!(
    PreconditionsFailedException,
    SCHEMA_PRECONDITIONS_FAILED_EXCEPTION,
    "com.amazonaws.neptunedata#PreconditionsFailedException",
    "com.amazonaws.neptunedata",
    "PreconditionsFailedException",
);

neptune_error_shape!(
    TimeLimitExceededException,
    SCHEMA_TIME_LIMIT_EXCEEDED_EXCEPTION,
    "com.amazonaws.neptunedata#TimeLimitExceededException",
    "com.amazonaws.neptunedata",
    "TimeLimitExceededException",
);

neptune_error_shape!(
    TooManyRequestsException,
    SCHEMA_TOO_MANY_REQUESTS_EXCEPTION,
    "com.amazonaws.neptunedata#TooManyRequestsException",
    "com.amazonaws.neptunedata",
    "TooManyRequestsException",
);

neptune_error_shape!(
    UnsupportedOperationException,
    SCHEMA_UNSUPPORTED_OPERATION_EXCEPTION,
    "com.amazonaws.neptunedata#UnsupportedOperationException",
    "com.amazonaws.neptunedata",
    "UnsupportedOperationException",
);

neptune_error_shape!(
    AccessDeniedException,
    SCHEMA_ACCESS_DENIED_EXCEPTION,
    "com.amazonaws.neptunedata#AccessDeniedException",
    "com.amazonaws.neptunedata",
    "AccessDeniedException",
);

neptune_error_shape!(
    CancelledByMaintenanceException,
    SCHEMA_CANCELLED_BY_MAINTENANCE_EXCEPTION,
    "com.amazonaws.neptunedata#CancelledByMaintenanceException",
    "com.amazonaws.neptunedata",
    "CancelledByMaintenanceException",
);

neptune_error_shape!(
    CancelledByUserException,
    SCHEMA_CANCELLED_BY_USER_EXCEPTION,
    "com.amazonaws.neptunedata#CancelledByUserException",
    "com.amazonaws.neptunedata",
    "CancelledByUserException",
);

neptune_error_shape!(
    InternalFailureException,
    SCHEMA_INTERNAL_FAILURE_EXCEPTION,
    "com.amazonaws.neptunedata#InternalFailureException",
    "com.amazonaws.neptunedata",
    "InternalFailureException",
);

neptune_error_shape!(
    MethodNotAllowedException,
    SCHEMA_METHOD_NOT_ALLOWED_EXCEPTION,
    "com.amazonaws.neptunedata#MethodNotAllowedException",
    "com.amazonaws.neptunedata",
    "MethodNotAllowedException",
);

neptune_error_shape!(
    ReadOnlyViolationException,
    SCHEMA_READ_ONLY_VIOLATION_EXCEPTION,
    "com.amazonaws.neptunedata#ReadOnlyViolationException",
    "com.amazonaws.neptunedata",
    "ReadOnlyViolationException",
);

neptune_error_shape!(
    ServerShutdownException,
    SCHEMA_SERVER_SHUTDOWN_EXCEPTION,
    "com.amazonaws.neptunedata#ServerShutdownException",
    "com.amazonaws.neptunedata",
    "ServerShutdownException",
);

neptune_error_shape!(
    ValidationException,
    SCHEMA_VALIDATION_EXCEPTION,
    "com.amazonaws.neptunedata#ValidationException",
    "com.amazonaws.neptunedata",
    "ValidationException",
);

neptune_error_shape!(
    BulkLoadIdNotFoundException,
    SCHEMA_BULK_LOAD_ID_NOT_FOUND_EXCEPTION,
    "com.amazonaws.neptunedata#BulkLoadIdNotFoundException",
    "com.amazonaws.neptunedata",
    "BulkLoadIdNotFoundException",
);

neptune_error_shape!(
    LoadUrlAccessDeniedException,
    SCHEMA_LOAD_URL_ACCESS_DENIED_EXCEPTION,
    "com.amazonaws.neptunedata#LoadUrlAccessDeniedException",
    "com.amazonaws.neptunedata",
    "LoadUrlAccessDeniedException",
);

neptune_error_shape!(
    MLResourceNotFoundException,
    SCHEMA_ML_RESOURCE_NOT_FOUND_EXCEPTION,
    "com.amazonaws.neptunedata#MLResourceNotFoundException",
    "com.amazonaws.neptunedata",
    "MLResourceNotFoundException",
);

neptune_error_shape!(
    InvalidNumericDataException,
    SCHEMA_INVALID_NUMERIC_DATA_EXCEPTION,
    "com.amazonaws.neptunedata#InvalidNumericDataException",
    "com.amazonaws.neptunedata",
    "InvalidNumericDataException",
);

neptune_error_shape!(
    MalformedQueryException,
    SCHEMA_MALFORMED_QUERY_EXCEPTION,
    "com.amazonaws.neptunedata#MalformedQueryException",
    "com.amazonaws.neptunedata",
    "MalformedQueryException",
);

neptune_error_shape!(
    MemoryLimitExceededException,
    SCHEMA_MEMORY_LIMIT_EXCEEDED_EXCEPTION,
    "com.amazonaws.neptunedata#MemoryLimitExceededException",
    "com.amazonaws.neptunedata",
    "MemoryLimitExceededException",
);

neptune_error_shape!(
    QueryLimitExceededException,
    SCHEMA_QUERY_LIMIT_EXCEEDED_EXCEPTION,
    "com.amazonaws.neptunedata#QueryLimitExceededException",
    "com.amazonaws.neptunedata",
    "QueryLimitExceededException",
);

neptune_error_shape!(
    QueryLimitException,
    SCHEMA_QUERY_LIMIT_EXCEPTION,
    "com.amazonaws.neptunedata#QueryLimitException",
    "com.amazonaws.neptunedata",
    "QueryLimitException",
);

neptune_error_shape!(
    QueryTooLargeException,
    SCHEMA_QUERY_TOO_LARGE_EXCEPTION,
    "com.amazonaws.neptunedata#QueryTooLargeException",
    "com.amazonaws.neptunedata",
    "QueryTooLargeException",
);

#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#QueryEvalStats")]
#[derive(Debug, Clone, Default)]
pub struct QueryEvalStats {
    pub waited: Option<i32>,
    pub elapsed: Option<i32>,
    pub cancelled: Option<bool>,
    pub subqueries: Option<::aws_smithy_types::Document>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#MlResourceDefinition")]
#[derive(Debug, Clone, Default)]
pub struct MlResourceDefinition {
    pub name: Option<String>,
    pub arn: Option<String>,
    pub status: Option<String>,
    pub output_location: Option<String>,
    pub failure_reason: Option<String>,
    pub cloudwatch_log_url: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#MlConfigDefinition")]
#[derive(Debug, Clone, Default)]
pub struct MlConfigDefinition {
    pub name: Option<String>,
    pub arn: Option<String>,
}

neptune_error_shape!(
    ExpiredStreamException,
    SCHEMA_EXPIRED_STREAM_EXCEPTION,
    "com.amazonaws.neptunedata#ExpiredStreamException",
    "com.amazonaws.neptunedata",
    "ExpiredStreamException",
);

neptune_error_shape!(
    StreamRecordsNotFoundException,
    SCHEMA_STREAM_RECORDS_NOT_FOUND_EXCEPTION,
    "com.amazonaws.neptunedata#StreamRecordsNotFoundException",
    "com.amazonaws.neptunedata",
    "StreamRecordsNotFoundException",
);

neptune_error_shape!(
    ThrottlingException,
    SCHEMA_THROTTLING_EXCEPTION,
    "com.amazonaws.neptunedata#ThrottlingException",
    "com.amazonaws.neptunedata",
    "ThrottlingException",
);

neptune_error_shape!(
    StatisticsNotAvailableException,
    SCHEMA_STATISTICS_NOT_AVAILABLE_EXCEPTION,
    "com.amazonaws.neptunedata#StatisticsNotAvailableException",
    "com.amazonaws.neptunedata",
    "StatisticsNotAvailableException",
);

#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#GremlinQueryStatus")]
#[derive(Debug, Clone, Default)]
pub struct GremlinQueryStatus {
    pub query_id: Option<String>,
    pub query_string: Option<String>,
    pub query_eval_stats: Option<QueryEvalStats>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.neptunedata#RefreshStatisticsIdMap")]
#[derive(Debug, Clone, Default)]
pub struct RefreshStatisticsIdMap {
    pub statistics_id: Option<String>,
}

/// The values `Action` accepts on the wire.
pub const ACTION_VALUES: &[&str] = &["initiateDatabaseReset", "performDatabaseReset"];


/// The values `Encoding` accepts on the wire.
pub const ENCODING_VALUES: &[&str] = &["gzip"];


/// The values `Format` accepts on the wire.
pub const FORMAT_VALUES: &[&str] = &["csv", "nquads", "ntriples", "opencypher", "rdfxml", "turtle"];


/// The values `GraphSummaryType` accepts on the wire.
pub const GRAPH_SUMMARY_TYPE_VALUES: &[&str] = &["basic", "detailed"];


/// The values `IteratorType` accepts on the wire.
pub const ITERATOR_TYPE_VALUES: &[&str] = &["AFTER_SEQUENCE_NUMBER", "AT_SEQUENCE_NUMBER", "LATEST", "TRIM_HORIZON"];


/// The values `Mode` accepts on the wire.
pub const MODE_VALUES: &[&str] = &["AUTO", "NEW", "RESUME"];


/// The values `OpenCypherExplainMode` accepts on the wire.
pub const OPEN_CYPHER_EXPLAIN_MODE_VALUES: &[&str] = &["details", "dynamic", "static"];


/// The values `Parallelism` accepts on the wire.
pub const PARALLELISM_VALUES: &[&str] = &["HIGH", "LOW", "MEDIUM", "OVERSUBSCRIBE"];


/// The values `RebuildStatus` accepts on the wire.
pub const REBUILD_STATUS_VALUES: &[&str] = &["CANCELLED", "CANCELLING", "COMPLETED", "FAILED", "PREPARING", "REBUILDING"];


/// The values `S3BucketRegion` accepts on the wire.
pub const S3_BUCKET_REGION_VALUES: &[&str] = &["af-south-1", "ap-east-1", "ap-east-2", "ap-northeast-1", "ap-northeast-2", "ap-northeast-3", "ap-south-1", "ap-south-2", "ap-southeast-1", "ap-southeast-2", "ap-southeast-3", "ap-southeast-4", "ap-southeast-5", "ap-southeast-7", "ca-central-1", "ca-west-1", "cn-north-1", "cn-northwest-1", "eu-central-1", "eu-central-2", "eu-north-1", "eu-south-2", "eu-west-1", "eu-west-2", "eu-west-3", "il-central-1", "me-central-1", "me-south-1", "mx-central-1", "sa-east-1", "us-east-1", "us-east-2", "us-gov-east-1", "us-gov-west-1", "us-west-1", "us-west-2"];


/// The values `Scope` accepts on the wire.
pub const SCOPE_VALUES: &[&str] = &["ALL", "DICTIONARIES", "QUADS"];


/// The values `StatisticsAutoGenerationMode` accepts on the wire.
pub const STATISTICS_AUTO_GENERATION_MODE_VALUES: &[&str] = &["disableAutoCompute", "enableAutoCompute", "refresh"];


/// The values `WriteBufferSize` accepts on the wire.
pub const WRITE_BUFFER_SIZE_VALUES: &[&str] = &["HIGH", "LOW", "MEDIUM"];


