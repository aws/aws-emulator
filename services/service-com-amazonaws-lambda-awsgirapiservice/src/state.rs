// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambda-awsgirapiservice service.
//!
//! The function lifecycle slice keeps every Lambda function this emulator holds in one
//! ordered map, so `ListFunctions` paginates deterministically and every operation
//! answers from the single record `CreateFunction` persisted.
//!
//! A function has one lifetime of its own. The model's own `CreateFunction`
//! documentation says: "When you create a function, Lambda provisions an instance of
//! the function and its supporting resources. [...] During this time, you can't invoke
//! or modify the function. The State, StateReason, and StateReasonCode fields in the
//! response from GetFunctionConfiguration indicate when the function is ready to
//! invoke." So a freshly created function is `Pending`, and the move to `Active` is
//! owed by the CLOCK rather than by anybody calling: the record stores the instant the
//! activation becomes due and [`FunctionRecord::materialize`] brings it up to whatever
//! the clock already made due. Nothing sleeps, and no read invents a transition that
//! was not owed.
//!
//! An update to a function is owed the same way. `UpdateFunctionCode` and
//! `UpdateFunctionConfiguration` report `LastUpdateStatus` `InProgress` on the call that
//! accepted the change, and [`FunctionRecord::begin_update`] stores the instant the move
//! to `Successful` becomes due, which `materialize` applies exactly as it applies the
//! move to `Active`.
//!
//! The resource-policy slice hangs off the same record. A function's resource-based
//! policy is attached to the ARN the permission names, so [`FunctionRecord::policies`]
//! holds one [`PolicyRecord`] per qualifier a statement was added to — the
//! function-level policy under [`VERSION_LATEST`], and one per published version or
//! alias. `AddPermission` appends exactly the statement its request described and
//! `GetPolicy` reports the document those statements make up, so a statement is never
//! echoed back out of a request body. `PutResourcePolicy` addresses the same policy by
//! ARN and supplies the WHOLE document instead ("This operation replaces any existing
//! policy on the Lambda resource"), so it stores that document on the same
//! [`PolicyRecord`] ([`PolicyRecord::of_document`]) and `GetResourcePolicy` and
//! `DeleteResourcePolicy` report and remove exactly what is stored there: both surfaces
//! read ONE policy per resource rather than a copy each.
//!
//! The asynchronous-invocation slice hangs off the same record. `InvokeAsync` answers
//! "202 Accepted ... the request is queued for invocation", so what it persists is the
//! queued event itself — on [`FunctionRecord::async_invocations`], one
//! [`AsyncInvocationRecord`] per accepted event, against the version it was addressed
//! to. What FOLLOWS the accepting call is the delivery: the event is handed to the
//! function and its handler runs, through the same in-process engine a synchronous
//! invocation runs on, so whatever the handler performs becomes observable shortly after
//! the call that queued it (see [`async_invocation::deliver`]). A delivery whose handler
//! ERRORED is attempted again, up to the attempts Lambda documents for one, and the entry
//! keeps what the last attempt came to; nothing invents a result, a log tail or a duration
//! for a function that did not run.
//!
//! The durable-execution slice hangs off the same record too, in
//! [`FunctionRecord::durable_executions`]: one [`DurableExecutionRecord`] per execution
//! started against the function, keyed by the execution name, which "must be unique
//! within the function". `ListDurableExecutionsByFunction` reports those records —
//! filtered and paged exactly as its own members ask — and never an execution nothing
//! started. An execution has a lifetime of its own: it starts `RUNNING`, and the move to
//! `TIMED_OUT` is owed by the CLOCK once the `ExecutionTimeout` of the function's
//! `DurableConfig` falls due (see [`DurableExecutionRecord::materialize`]). The move to
//! `STOPPED` is the one a CALLER makes: `StopDurableExecution` ends the execution it
//! addresses at the instant of the call and keeps the reason the call gave (see
//! [`DurableExecutionRecord::stop`]).
//!
//! The runtime-management slice hangs off the same record, one entry per version:
//! `PutRuntimeManagementConfig` "sets the runtime management configuration for a
//! function's version", so what it persists lives on
//! [`FunctionRecord::runtime_management_configs`] keyed by the version label the call
//! addressed, and `GetRuntimeManagementConfig` reports exactly that. A version nobody
//! has configured carries the documented default rather than nothing at all (see
//! [`RuntimeManagementConfigRecord::default`]).
//!
//! The capacity-provider slice is the other resource this store holds, in
//! [`ComAmazonawsLambdaAwsgirapiserviceState::capacity_providers`]: one
//! [`CapacityProviderRecord`] per provider `CreateCapacityProvider` persisted, keyed by
//! its name so `ListCapacityProviders` pages through them deterministically. A provider
//! has a lifetime of its own too — it is created `Pending` and the move to `Active` is
//! owed by the CLOCK (see [`CapacityProviderRecord::materialize`]) — and every member
//! reported for it comes out of the record.
//!
//! The reserved-concurrency slice is one value on the same record:
//! `PutFunctionConcurrency` "Sets the maximum number of simultaneous executions for a
//! function, and reserves capacity for that concurrency level", and "Concurrency settings
//! apply to the function as a whole, including all published versions and the unpublished
//! version", so what it persists is the single
//! [`FunctionRecord::reserved_concurrent_executions`] of the function it addressed.
//! `GetFunctionConcurrency` and `GetFunction` report exactly that value, and the account's
//! unreserved capacity is what every allocation it has committed to an individual function
//! leaves — reservations and the provisioned concurrency of the functions that reserve
//! nothing alike (see
//! [`ComAmazonawsLambdaAwsgirapiserviceState::allocated_concurrency`] and
//! [`MIN_UNRESERVED_CONCURRENT_EXECUTIONS`]).
//!
//! The provisioned-concurrency slice hangs off the same record, one entry per qualifier:
//! `PutProvisionedConcurrencyConfig` "Adds a provisioned concurrency configuration to a
//! function's alias or version", so what it persists lives on
//! [`FunctionRecord::provisioned_concurrency_configs`] keyed by the qualifier the call
//! addressed, and every reader reports exactly that record. The allocation it asks for is
//! owed by the CLOCK rather than by anybody calling — a configuration is stored
//! [`PROVISIONED_CONCURRENCY_IN_PROGRESS`] with nothing allocated yet, and
//! [`ProvisionedConcurrencyRecord::materialize`] brings it up to whatever the clock has
//! already made due.
//!
//! The function-scaling slice hangs off the same record, one entry per qualifier:
//! `PutFunctionScalingConfig` "Sets the scaling configuration for a Lambda Managed
//! Instances function" for the version or published view its `Qualifier` names, so what
//! it persists lives on [`FunctionRecord::function_scaling_configs`] keyed by that
//! qualifier, and `GetFunctionScalingConfig` reports exactly that record. The bounds the
//! call asked for are the REQUESTED configuration at once, and become the APPLIED one —
//! "the actual scaling settings in effect" — when the CLOCK makes that due (see
//! [`FunctionScalingConfigRecord::materialize`]); nothing sleeps for it. Once they are
//! applied there is no outstanding request left to report, so the requested configuration
//! is reported only while one is (see [`FunctionScalingConfigRecord::reported`]). A
//! qualifier nobody has configured still reports what the SERVICE scales it by, because a
//! Managed Instances function is scaled from the moment it exists (see
//! [`FunctionRecord::reported_function_scaling_config`]), and a qualifier that runs on no
//! capacity provider is outside the surface both operations serve, so the call is refused
//! rather than answered for (see
//! [`FunctionRecord::require_capacity_provider_configuration`]).
//!
//! The asynchronous-invocation-configuration slice hangs off the same record, one entry per
//! qualifier: `PutFunctionEventInvokeConfig` "Configures options for asynchronous
//! invocation on a function, version, or alias", so what it persists lives on
//! [`FunctionRecord::event_invoke_configs`] keyed by the qualifier the call addressed, and
//! `GetFunctionEventInvokeConfig` and `ListFunctionEventInvokeConfigs` report exactly those
//! records. A qualifier nobody has configured holds NO configuration, which is what the
//! reads answer the modelled 404 for (see [`no_event_invoke_config_message`]); the put
//! replaces an addressed qualifier's configuration whole and the update changes only the
//! settings its request carries (see [`EventInvokeConfigRecord`]).
//!
//! What those same records come to at the account level is also METERED, with no client
//! action at all: Lambda publishes how much of the Region's concurrency the account has
//! claimed and how much of its unreserved pool is in use, which is what an operator alarms
//! on the account's remaining headroom from. An execution of a function that reserves
//! concurrency is charged to that reservation and never to the unreserved pool, so the
//! difference between the two series is exactly the concurrency the account has allocated;
//! see [`concurrency_metering`], which samples these records and publishes into the
//! CloudWatch store [`metrics`] publishes the per-function series into.
//!
//! `GetAccountSettings` reports on the same records at the account level:
//! "Retrieves details about your account's limits and usage in an Amazon Web Services
//! Region", so its usage is what the store holds and its unreserved concurrency is what
//! those records reserve (see
//! [`ComAmazonawsLambdaAwsgirapiserviceState::account_usage`] and
//! [`ComAmazonawsLambdaAwsgirapiserviceState::account_limit`]).
//!
//! The code-signing association is one value on the same record:
//! `PutFunctionCodeSigningConfig` "Update[s] the code signing configuration for the
//! function", and a function has one such configuration at a time, so what it persists is
//! the single [`FunctionRecord::code_signing_config_arn`] of the function it addressed —
//! the ARN of a configuration this store already holds, never one the request invented.
//! `CreateFunction` sets the same value from its own `CodeSigningConfigArn` member, and
//! `GetFunctionCodeSigningConfig` reports exactly what is stored there, so one function
//! has ONE association however it was set. `DeleteFunctionCodeSigningConfig` "Removes the
//! code signing configuration from the function", so it clears that same single value and
//! the function then holds none. The association is also what makes a stored
//! configuration IN USE: `DeleteCodeSigningConfig` can remove a configuration "only if no
//! function is using it", so it removes the record out of
//! [`ComAmazonawsLambdaAwsgirapiserviceState::code_signing_configs`] only when
//! [`ComAmazonawsLambdaAwsgirapiserviceState::code_signing_config_users`] finds no
//! function naming it.
//!
//! The capacity-provider slice is the account's other top-level resource:
//! [`ComAmazonawsLambdaAwsgirapiserviceState::capacity_providers`] holds one
//! [`CapacityProviderRecord`] per provider, keyed by its bare name, so every operation
//! that reports a provider answers from the single record `CreateCapacityProvider`
//! persisted. A provider has a lifetime of its own in exactly the way a function does:
//! the model's `CapacityProviderState` enum carries `Pending` beside `Active`, so a
//! freshly created provider is `Pending` and the move to `Active` is owed by the CLOCK
//! rather than by anybody calling — the record stores the instant that move becomes due
//! and [`CapacityProviderRecord::materialize`] brings it up to whatever the clock has
//! already made due. `UpdateCapacityProvider` rewrites the configuration members its
//! request states on that same record and leaves the ones it omits alone, and
//! `DeleteCapacityProvider` moves it into the enum's `Deleting` value with the instant
//! its removal becomes due ([`CapacityProviderRecord::begin_delete`]), which the clock
//! then owes exactly as it owes the activation.
//!
//! The event-source-mapping slice is
//! [`ComAmazonawsLambdaAwsgirapiserviceState::event_source_mappings`]: one
//! [`EventSourceMappingRecord`] per mapping, keyed by its UUID, with the same clock-owed
//! lifetime (`Creating` and `Updating` settle into the state the mapping's `Enabled`
//! setting asks for, and a deletion's removal falls due). A mapping that reports `Enabled`
//! also POLLS its event source, because "Lambda reads items from the event source and
//! invokes the function": for an Amazon SQS queue that reading is [`sqs_poller`], which
//! takes a batch out of the queue through the queue's own delivery bookkeeping and records
//! what became of it on [`EventSourceMappingRecord::last_processing_result`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use sha2::Digest as _;

use crate::ops::types::{
    AccountLimit, AccountUsage, AliasRoutingConfiguration, AllowedPublishers, CapacityProvider,
    CapacityProviderConfig, CapacityProviderPermissionsConfig, CapacityProviderScalingConfig,
    CapacityProviderTelemetryConfig, CapacityProviderVpcConfig, CodeSigningConfig,
    CodeSigningPolicies, Cors, CreateFunctionOutput, DeadLetterConfig, DurableConfig, Environment,
    ErrorObject, Event, EventError, EventInput, EventResult, ExecutionFailedDetails,
    ExecutionStoppedDetails, ExecutionSucceededDetails, ExecutionTimedOutDetails,
    FileSystemConfig, FunctionScalingConfig, GetFunctionScalingConfigOutput, GetLayerVersionOutput,
    GetProvisionedConcurrencyConfigOutput, ImageConfig, InstanceRequirements,
    LayerVersionContentOutput, LayerVersionsListItem, LoggingConfig, PropagateTags,
    ProvisionedConcurrencyConfigListItem, ResolvedS3Object, SelfManagedEventSource, TenancyConfig,
    VpcConfig,
};

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `lambda`"). It is also the vendor field a client-sent
/// ARN must carry to name one of this service's resources, which is what the handlers
/// that accept a name-or-ARN member check it against.
pub const ARN_NAMESPACE: &str = "lambda";

/// How a capacity provider is addressed — the ARN this service mints for one, the ARN
/// resource type it carries, the reading of a client-sent `CapacityProviderName` and the
/// refusal a value naming no provider gets — lives in [`capacity_provider`] and is
/// re-exported here, so a handler still names those as `crate::state::…`. The record, its
/// clock-owed transition and the store that holds it stay in this module, beside the
/// function slice the tagging surface reaches them through.
pub mod capacity_provider;
pub use capacity_provider::{
    capacity_provider_arn, capacity_provider_name_message, parse_capacity_provider_name,
    CAPACITY_PROVIDER_RESOURCE_TYPE,
};

/// What an event source mapping does with NO client action at all — the pollers that read
/// the mapped source and hand each batch to the function — lives beside the record, one
/// module per KIND of source, because what a source hands over and how a reader keeps its
/// place in it differ: [`sqs_poller`] for a mapping on an Amazon SQS queue, and
/// [`event_source_poller`] for one on a SHARD source — an Amazon Kinesis data stream or an
/// Amazon DynamoDB table's stream — whose shards are read from a position the mapping
/// carries. The CloudWatch series Lambda publishes about that work is in [`metrics`]. Each
/// is re-exported here for the same reason as [`capacity_provider`]: the mapping record,
/// its clock-owed transitions and the store that holds it stay in this module, and a
/// handler still names a poller as `crate::state::…`.
pub mod event_source_poller;
pub mod metrics;
pub use metrics::{invoked_resource, publish_invocation, publish_throttle, InvocationMetrics};
pub mod sqs_poller;
pub use sqs_poller::{
    batch_is_due, claim_poll_passes, drive_sqs_pollers, poll_due, poll_passes_once, PollSession,
    POLL_INTERVAL,
};

/// Which records of a batch the function it was handed to reported it did not process — the
/// ONE reading of a partial batch response, which both pollers above decide what to consume
/// by — lives in [`batch_item_failures`], beside them. It is re-exported here for the same
/// reason as [`capacity_provider`]: the mapping record carrying the `FunctionResponseTypes`
/// that asks for the report to be read stays in this module.
pub mod batch_item_failures;
pub use batch_item_failures::{
    processed_records, read_batch_item_failures, reports_batch_item_failures, BatchReport,
    REPORT_BATCH_ITEM_FAILURES,
};

/// Whether the execution role of the function a mapping invokes may READ the event source
/// that mapping names — the gate every call that wires the two together passes through —
/// lives in [`event_source_authorization`], beside the pollers that make those reads. It is
/// re-exported here for the same reason as [`capacity_provider`]: a handler names it as
/// `crate::state::…`.
pub mod event_source_authorization;
pub use event_source_authorization::unreadable_event_source;

/// An event source carries a resource-based policy of its OWN, held by the service that owns
/// the source: reading it out of that service's store lives in
/// [`event_source_resource_policy`], beside the decision above that it is half of.
pub mod event_source_resource_policy;
pub use event_source_resource_policy::SourcePolicies;

/// Whether a function's execution role may do what attaching it to a VPC takes lives in
/// [`vpc_attachment_authorization`], beside the reading above, and is re-exported for the
/// same reason: a handler names it as `crate::state::…`.
pub mod vpc_attachment_authorization;
pub use vpc_attachment_authorization::unattachable_vpc_config;

/// Following a container-image package's reference into the Amazon ECR registry — whether
/// it names an image at all, whether that registry lets this service pull it, and which
/// image bytes the function is frozen to — lives in [`image_source`], reached through the
/// same shared engine as the stores above and re-exported for the same reason: a handler
/// names it as `crate::state::…`.
pub mod image_source;
pub use image_source::{
    emulator_registry_authorities, image_package_digest, resolve_image_source,
    source_image_message, ImageResolution, ImageSourceRefusal, IMAGE_ACCESS_DENIED_MESSAGE,
};

/// Following a .zip package's Amazon S3 location — whether Amazon S3 hands the object over,
/// and which bytes the package is — lives in [`s3_source`], beside the reference-following
/// above and re-exported for the same reason: the three writes that store such a package
/// name it as `crate::state::…`.
pub mod s3_source;
pub use s3_source::{read_s3_package, s3_package_bytes, s3_package_message, S3Package};

/// Asking that question for the SET of calls one such thing takes — which of them the role
/// may not make — lives in [`role_actions`], so the gates that each take several calls ask it
/// the same way over the one decision above.
pub(crate) mod role_actions;

/// Whether IAM lets the principal an INVOCATION speaks for invoke the function it names — the
/// decision every invoking operation takes before the function is reached — lives in
/// [`invoke_authorization`], beside the two readings above. It is re-exported here for the
/// same reason as [`capacity_provider`]: the function record whose resource-based policy it
/// reads, and the store that holds it, stay in this module.
pub mod invoke_authorization;
pub use invoke_authorization::{
    authorize_invocation, INVOKE_ASYNC_ACTION, INVOKE_FUNCTION_ACTION,
};

/// The ASYNCHRONOUS-INVOCATION CONFIGURATION of a function, version or alias — the record
/// the `…FunctionEventInvokeConfig` operations read and write, and whether the function's
/// execution role lets Lambda deliver a record to the destinations one names — lives in
/// [`event_invoke_config`], beside the execution-role reading above it shares its evidence
/// with. It is re-exported here for the same reason as [`capacity_provider`]: the records
/// hang off the function record this module holds.
pub mod event_invoke_config;
pub use event_invoke_config::{
    configured_function_arn, no_event_invoke_config_message, requested_destinations,
    unwritable_destination, EventInvokeConfigRecord,
};

/// What the account's CONCURRENCY does with no client action at all — the series Lambda
/// publishes about how much of the Region's capacity the account has claimed, and how much
/// of its unreserved pool is in use — lives in [`concurrency_metering`], beside the
/// per-function series in [`metrics`] it shares the CloudWatch store with. It is re-exported
/// here for the same reason as [`capacity_provider`]: the records it reads, and the store
/// that holds them, stay in this module.
pub mod concurrency_metering;
pub use concurrency_metering::{
    drive_concurrency_metering, publish_account_concurrency, publish_function_concurrency,
    AccountConcurrency,
};

/// What Lambda's RECURSIVE-LOOP guardrail counts, when it stops an invocation and what it
/// meters when it does lives in [`recursion`], beside the per-function series in [`metrics`]
/// it publishes through. It is re-exported here for the same reason as
/// [`capacity_provider`]: the function record it counts on, and the store that holds it,
/// stay in this module.
pub mod recursion;
pub use recursion::{
    guardrail, publish_recursive_invocation_dropped, request_chain, Guardrail,
    RequestChainRecord, MAX_REQUEST_CHAIN_INVOCATIONS, RECURSIVE_INVOCATION_MESSAGE,
    RECURSIVE_LOOP_ALLOW, RECURSIVE_LOOP_TERMINATE,
};

/// RUNNING a function — what an invocation of the deployment package this store holds comes
/// to, and everything that bounds and contains that run — lives in [`execution`], beside the
/// record the package is read off. It is re-exported here for the same reason as
/// [`capacity_provider`]: the record it reads, and the store that holds it, stay in this
/// module.
pub mod execution;
pub use execution::{
    durable_data_key_fault, engine_for, environment_key_fault, function_sdk_access,
    invocation_trace_context, layer_archives, log_target, mint_request_id, run_invocation,
    runs_handler, supplied_trace_header, AwsAccess, Engine, EnvironmentKeyFault, Invocation,
    Invoked, LogTarget, Unrunnable, FUNCTION_ERROR_UNHANDLED,
};

/// How large a stored .zip archive is once EXPANDED — the second size Lambda reports for a
/// layer version and a deployment package — lives in [`archive`], beside the reading of the
/// same archive's entries in [`execution::package`]. It is re-exported here for the same
/// reason as [`capacity_provider`]: the records holding those archives stay in this module.
pub mod archive;
pub use archive::{is_over_the_unzipped_limit, unzipped_bytes, unzipped_limit_message};

/// Whether an invocation's payload is the JSON an invocation carries, and the diagnostic a
/// payload that is not is refused with, lives in [`invocation_payload`], beside the running
/// of the invocation in [`execution`]. It is re-exported here for the same reason as
/// [`capacity_provider`]: every operation that takes a payload names it as
/// `crate::state::…`.
pub mod invocation_payload;
pub use invocation_payload::unparseable_payload_message;

/// WHERE a function's invocations are logged — the one reading of `LoggingConfig$LogGroup`
/// and of the default group — lives in [`invocation_logs`], between the read that REPORTS
/// that group and the delivery that WRITES to it. It is re-exported here for the same reason
/// as [`capacity_provider`]: both of those name it as `crate::state::…`.
pub mod invocation_logs;
pub use invocation_logs::configured_log_group;

/// What the RUN of a durable function's invocation does to the EXECUTION that invocation
/// started lives in [`durable_run`], beside the running of the invocation in [`execution`]
/// and the queue that drives it in [`async_invocation`]. The record it settles stays in this
/// module.
pub mod durable_run;

/// The EventBridge events a durable execution's status changes OWE the account's default
/// event bus live in [`durable_events`], beside the record whose statuses they report: one
/// event per status an execution moves into, published off the response path, with the clock
/// that keeps publishing for the transitions nobody will ask for.
pub mod durable_events;

/// Which stored function URL a request ADDRESSES — the reading behind the data plane that
/// serves the endpoints [`function_url()`] mints — lives in the [`function_url`] module,
/// beside the [`FunctionUrlConfigRecord`]s it resolves. It is re-exported here for the same
/// reason as [`capacity_provider`]: the plane in `crate::service` names it as
/// `crate::state::…`.
pub mod function_url;
pub use function_url::{AddressedFunctionUrl, FunctionUrlTarget};

/// The workspace's one reading of an authenticated pagination token, which this store's
/// paginated reads mint their `NextMarker` with
/// ([`function_url::function_url_marker`]): the secret lives on the store
/// ([`ComAmazonawsLambdaAwsgirapiserviceState::page_token_key`]), so a token one store
/// issued is the only one it reads back.
pub use ::service_common::page_tokens::PageTokenKey;

/// The region an ARN carries when the request is unsigned and therefore names no
/// SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The unpublished version label every function always has, as the model's `Version`
/// pattern (`^(\$LATEST|[0-9]+)$`) and `CreateFunction`'s documentation ("A function
/// has an unpublished version") name it.
pub const VERSION_LATEST: &str = "$LATEST";

/// `PackageType` values, from the model's `PackageType` enum (`Zip | Image`).
pub const PACKAGE_TYPE_ZIP: &str = "Zip";
/// See [`PACKAGE_TYPE_ZIP`].
pub const PACKAGE_TYPE_IMAGE: &str = "Image";

/// `State` values this slice moves a function through, from the model's `State` enum.
pub const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
pub const STATE_ACTIVE: &str = "Active";

/// The `State` a settled function reports while its unpublished configuration cannot be
/// invoked directly, from the same `State` enum (`Pending | Active | Inactive | Failed |
/// Deactivating | Deactivated | ActiveNonInvocable | Deleting`).
///
/// A function that names a capacity provider is such a function: the compute it runs on
/// belongs to the provider, and what the provider runs is a published version registered
/// with it, so the function is provisioned and healthy without `$LATEST` being invocable.
/// See [`FunctionRecord::reported_state`].
pub const STATE_ACTIVE_NON_INVOCABLE: &str = "ActiveNonInvocable";

/// The `StateReasonCode` a function carries while it is being provisioned, from the
/// model's `StateReasonCode` enum, whose documentation reads: "When the code is
/// Creating, you can't invoke or modify the function."
pub const STATE_REASON_CODE_CREATING: &str = "Creating";

/// The `StateReason` that accompanies [`STATE_REASON_CODE_CREATING`].
pub const STATE_REASON_CREATING: &str = "The function is being created.";

/// The `LastUpdateStatus` a settled function carries. The model documents the member
/// as "The status of the last update that was performed on the function. This is first
/// set to Successful after function creation completes", so it is absent while the
/// function is still `Pending`.
pub const LAST_UPDATE_STATUS_SUCCESSFUL: &str = "Successful";

/// The `LastUpdateStatus` a function carries while an update is still being applied,
/// from the model's `LastUpdateStatus` enum. `UpdateFunctionConfiguration`'s own
/// documentation describes the window it covers: "When you update a function, Lambda
/// provisions an instance of the function and its supporting resources. [...] During
/// this time, you can't modify the function, but you can still invoke it. The
/// LastUpdateStatus [...] fields in the response from GetFunctionConfiguration indicate
/// when the update is complete". A code update is a change to the same function, so
/// `UpdateFunctionCode` reports it too; see [`FunctionRecord::begin_update`].
pub const LAST_UPDATE_STATUS_IN_PROGRESS: &str = "InProgress";

/// How long after `CreateFunction` the move from `Pending` to `Active` becomes due.
///
/// An EMULATOR-CHOSEN span (the real provisioning time depends on the package and on
/// whether the function joins a VPC, and is not in the model). It is deliberately
/// non-zero, so a caller that acts immediately after `CreateFunction` still observes
/// the `Pending` state the model documents and the state errors that go with it.
pub const ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after an accepted update (of a function's code or of its configuration) the
/// move from [`LAST_UPDATE_STATUS_IN_PROGRESS`] to [`LAST_UPDATE_STATUS_SUCCESSFUL`]
/// becomes due.
///
/// An EMULATOR-CHOSEN span, for the same reason as [`ACTIVATION_DELAY`]: the real time
/// to provision a new instance of the function is not in the model ("If your function
/// connects to a VPC, this process can take a minute"). It has to OUTLAST the call that
/// opened it, because the window is what every client of this lifecycle reads: a caller
/// that reads the function back, or modifies it again, as its very next call still finds
/// [`LAST_UPDATE_STATUS_IN_PROGRESS`] and the refusal the same documentation describes
/// ("During this time, you can't modify the function"). A deployment package may be
/// megabytes, so that next call is issued seconds rather than milliseconds after the
/// update was accepted, and the span is sized for it.
pub const UPDATE_DELAY: Duration = Duration::from_secs(3);

/// Lambda's sentence for a modification refused because an update of the function it
/// names is still being applied — the window [`LAST_UPDATE_STATUS_IN_PROGRESS`] covers
/// ("During this time, you can't modify the function, but you can still invoke it").
///
/// It is the message of the `ResourceConflictException` ("another operation is in
/// progress", HTTP 409) that every operation modifying a function answers while that
/// window is open, so all of them say it the same way: fixed text, naming the function's
/// own unqualified ARN as it is stored ([`FunctionRecord::arn`]) rather than anything the
/// request carried.
#[must_use]
pub fn update_in_progress_message(function_arn: &str) -> String {
    format!(
        "The operation cannot be performed at this time. An update is in progress for \
         resource: {function_arn}"
    )
}

/// The `SnapStart$ApplyOn` value that asks Lambda to snapshot an initialized execution
/// environment, from the model's `SnapStartApplyOn` enum (`PublishedVersions | None`).
pub const SNAP_START_APPLY_ON_PUBLISHED_VERSIONS: &str = "PublishedVersions";

/// `SnapStartResponse$OptimizationStatus` values, from the model's
/// `SnapStartOptimizationStatus` enum (`On | Off`). The member reports, "When you
/// provide a qualified Amazon Resource Name (ARN), this response element indicates
/// whether SnapStart is activated for the specified function version", so a version
/// whose configuration applies SnapStart reports `On` and everything else reports `Off`.
pub const SNAP_START_OPTIMIZATION_STATUS_ON: &str = "On";
/// See [`SNAP_START_OPTIMIZATION_STATUS_ON`].
pub const SNAP_START_OPTIMIZATION_STATUS_OFF: &str = "Off";

/// The default `MemorySize`, in MB: "The default value is 128 MB", as AWS documents
/// for the member (the model carries only its 128..32768 range).
pub const DEFAULT_MEMORY_SIZE_MB: i32 = 128;

/// The default `Timeout`, in seconds: "The default is 3 seconds", as AWS documents for
/// the member (the model carries only its 1..5400 range).
pub const DEFAULT_TIMEOUT_SECONDS: i32 = 3;

/// The longest `Timeout` a function may carry, in seconds: AWS's published Lambda quota
/// "Function timeout: 900 seconds (15 minutes)".
///
/// The model's `Timeout` range is wider (1..5400), so this is the narrower of the two and
/// the one a request is refused against — the same way the recorded service refuses a
/// larger value, naming the member and the bound it failed.
pub const MAX_TIMEOUT_SECONDS: i32 = 900;

/// The largest `MemorySize` a function may carry, in MB: AWS's published Lambda quota
/// "Function memory allocation: 128 MB to 10,240 MB, in 1-MB increments".
///
/// The model's `MemorySize` range is wider (128..32768), so this is the narrower of the
/// two and the one a request is refused against, exactly as [`MAX_TIMEOUT_SECONDS`] is
/// for `Timeout`.
pub const MAX_MEMORY_SIZE_MB: i32 = 10_240;

/// How one over-quota member is refused: the sentence it carries.
///
/// Both quotas are NARROWER than the ranges this model declares (`Timeout` 1..5400,
/// `MemorySize` 128..32768), so the request walk's boundary validator lets an over-quota
/// value through and the handler is the only place that can refuse it: every operation
/// that accepts either member judges it here, before anything is stored, so a
/// configuration no function could run is never persisted and a later read never reports
/// one.
///
/// Each member keeps the wording the service answers for its own ceiling: the `Timeout`
/// clause quotes the rejected value and names the member path and the bound, while the
/// `MemorySize` sentence names the member. Neither carries the `N validation error(s)
/// detected: ` envelope the boundary walk builds when it aggregates the violations of one
/// request, because neither of these is a bound the model declares for that walk to
/// collect (see [`pattern_violation_message`] for the refusals that are) — but both state
/// a bound on the VALUE of a member, so both are answered through the same channel that
/// walk answers through: a 400 `ValidationException`
/// ([`crate::scaffolded::Scaffolded::validation`]).
///
/// Answers `None` when both members are inside their quota.
#[must_use]
pub fn configuration_quota_refusal(
    timeout: Option<i32>,
    memory_size: Option<i32>,
) -> Option<String> {
    if let Some(timeout) = timeout.filter(|seconds| *seconds > MAX_TIMEOUT_SECONDS) {
        return Some(format!(
            "Value '{timeout}' at 'timeout' failed to satisfy constraint: Member must have \
             value less than or equal to {MAX_TIMEOUT_SECONDS}"
        ));
    }
    if memory_size.is_some_and(|megabytes| megabytes > MAX_MEMORY_SIZE_MB) {
        return Some(format!(
            "'MemorySize' value failed to satisfy constraint: Member must have value less \
             than or equal to {MAX_MEMORY_SIZE_MB}"
        ));
    }
    None
}

/// The `LogFormat` a function gets when the caller names none: "The default log format is
/// plain text", as AWS documents for the member (the model carries only its `JSON | Text`
/// enum).
pub const DEFAULT_LOG_FORMAT: &str = "Text";

/// The `LogFormat` whose functions carry the LOG LEVELS as defaults.
///
/// `LoggingConfig$ApplicationLogLevel` and `$SystemLogLevel` are the level filters Lambda
/// applies to a function's logs, and AWS documents them as available with the JSON log
/// format ("To use this feature, set the log format to JSON") — so a function on that
/// format carries both, at [`DEFAULT_LOG_LEVEL`] until the caller sets them, while one on
/// the plain-text format carries only what was stored for it, filtering by no level.
pub const LOG_FORMAT_JSON: &str = "JSON";

/// The level both log-level members default to on a JSON-format function, which AWS
/// documents for each of them: "Default: INFO".
///
/// It is the level the function's logs are FILTERED at, so a caller reading the effective
/// configuration of a JSON-format function is told the levels that are in effect rather
/// than nothing at all.
pub const DEFAULT_LOG_LEVEL: &str = "INFO";

/// The `SnapStart` `ApplyOn` a function gets when the caller names none, from the model's
/// `SnapStartApplyOn` enum (`PublishedVersions | None`): no snapshot is taken.
pub const SNAP_START_APPLY_ON_NONE: &str = "None";

/// The default `EphemeralStorage` size, in MB, which the model's own member
/// documentation states: "The default value is 512, but can be any whole number
/// between 512 and 10,240 MB."
pub const DEFAULT_EPHEMERAL_STORAGE_MB: i32 = 512;

/// The default `Architectures` entry, which the model's own member documentation
/// states: "Enter a string array with one of the valid values (arm64 or x86_64). The
/// default value is x86_64."
pub const DEFAULT_ARCHITECTURE: &str = "x86_64";

/// The default `TracingConfig` mode, from the model's `TracingMode` enum
/// (`Active | PassThrough`); `PassThrough` is the mode a function gets when the caller
/// names no tracing configuration, because X-Ray then samples nothing of its own.
pub const DEFAULT_TRACING_MODE: &str = "PassThrough";

/// The other `TracingConfig` mode of that enum: "Set Mode to Active to sample and trace a
/// subset of incoming requests with X-Ray" (`TracingConfig$Mode`), which is what makes an
/// invocation of the function one X-Ray records a segment for — and so what decides the
/// trace context the run reports ([`execution::trace_context`]).
pub const TRACING_MODE_ACTIVE: &str = "Active";

/// The `RecursiveLoop` configuration a function has before anyone sets one, which the
/// model's own `RecursiveLoop` member documentation states twice — on
/// `GetFunctionRecursionConfig`'s output and on `PutFunctionRecursionConfig`'s input:
/// "By default, Lambda sets your function's configuration to Terminate."
pub const DEFAULT_RECURSIVE_LOOP: &str = recursion::RECURSIVE_LOOP_TERMINATE;

/// The `UpdateRuntimeOn` values a runtime management configuration may carry, from the
/// model's `UpdateRuntimeOn` enum (`Auto | Manual | FunctionUpdate`).
pub const UPDATE_RUNTIME_ON_AUTO: &str = "Auto";
/// See [`UPDATE_RUNTIME_ON_AUTO`]. In this mode "You specify a runtime version in your
/// function configuration", so it is the only mode that carries a `RuntimeVersionArn`.
pub const UPDATE_RUNTIME_ON_MANUAL: &str = "Manual";
/// See [`UPDATE_RUNTIME_ON_AUTO`].
pub const UPDATE_RUNTIME_ON_FUNCTION_UPDATE: &str = "FunctionUpdate";

/// The runtime update mode a function version has before anyone sets one, which the
/// model's own `UpdateRuntimeOn` member documentation states: "Auto (default) -
/// Automatically update to the most recent and secure runtime version".
pub const DEFAULT_UPDATE_RUNTIME_ON: &str = UPDATE_RUNTIME_ON_AUTO;

/// Lambda's sentence for a runtime management configuration that selects the manual
/// update mode without the runtime version ARN that mode requires.
///
/// The requirement is conditional, so the model cannot express it as a required member —
/// `RuntimeVersionArn` is documented as "This is only required if you're using the
/// Manual runtime update mode" — and Lambda diagnoses it with this one fixed sentence:
/// the member that is missing, the condition that makes it required, and what to do
/// about it. Every word of it is fixed: nothing of the request appears in it, and the
/// condition names the mode as the service's own sentence spells it.
pub const RUNTIME_VERSION_ARN_REQUIRED_MESSAGE: &str =
    "Missing runtime version ARN. A runtime version ARN is required when \
     UpdateRuntimeVersionOn=Manual. Provide a valid runtime version ARN and retry the \
     request.";

/// The `FunctionCodeLocation` `RepositoryType` of a .zip file archive package: Lambda
/// keeps the archive in its own S3 bucket and vends a download location for it.
pub const REPOSITORY_TYPE_S3: &str = "S3";
/// The `FunctionCodeLocation` `RepositoryType` of a container image package: the image
/// lives in the Amazon ECR registry.
pub const REPOSITORY_TYPE_ECR: &str = "ECR";

/// The longest a bare function name may be: "The length constraint applies only to the
/// full ARN. If you specify only the function name, it is limited to 64 characters in
/// length", as the model documents on `FunctionName`.
pub const MAX_BARE_FUNCTION_NAME_LEN: usize = 64;

/// The most entries `ListFunctions` puts in one page, which the model documents twice:
/// on the operation ("Lambda returns up to 50 functions per call") and on its
/// `MaxItems` member ("Note that ListFunctions returns a maximum of 50 items in each
/// response, even if you set the number higher"). It is therefore both the page size a
/// caller that names no `MaxItems` gets and the ceiling on the one who does.
pub const MAX_PAGE_SIZE: usize = 50;

/// The most functions this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `CreateFunction` adds to it, so past this point the slice answers the modelled
/// `CodeStorageExceededException` rather than growing without bound. AWS caps the
/// total code storage of an account and answers the same modelled error there.
pub const MAX_FUNCTIONS: usize = 1_000;

/// The `UntrustedArtifactOnDeployment` a code-signing configuration gets when the caller
/// names no policy, which the model's own member documentation states: "Default value:
/// `Warn`".
pub const DEFAULT_CODE_SIGNING_POLICY: &str = "Warn";

/// The most code-signing configurations this emulator keeps, from AWS's published Lambda
/// quota "Code signing configurations: 20 per account".
pub const MAX_CODE_SIGNING_CONFIGS: usize = 20;

/// The refusal an operation naming a code-signing configuration this account holds none
/// of answers with: the sentence AWS words `CodeSigningConfigNotFoundException` as, with
/// the ARN the request named as its variable part.
///
/// Every operation that takes a `CodeSigningConfigArn` naming a configuration that has to
/// already exist — `PutFunctionCodeSigningConfig` and `CreateFunction` — refuses with this
/// one sentence, so the association surface states the same thing wherever it is set.
#[must_use]
pub fn code_signing_config_not_found_message(arn: &str) -> String {
    format!(
        "The code signing configuration cannot be found. Check that the provided \
         configuration is not deleted: {arn}."
    )
}

/// The refusal `DeleteCodeSigningConfig` answers when some function still names the
/// configuration the request asked to delete.
///
/// The CONDITION is the one the operation documents — "You can delete the code signing
/// configuration only if no function is using it" — and the SENTENCE below is how the
/// service words that refusal, as the recorded response to such a delete carries it: it is
/// reproduced verbatim rather than reworded to the condition, because the wording is the
/// requirement. The service classifies the association as the modelled
/// `ResourceConflictException` ("The resource already exists, or another operation is in
/// progress") and words the refusal in those terms.
///
/// It is the WHOLE message: no ARN, no function and no count, so there is nothing of the
/// request in it and a caller can compare it exactly. Which functions hold the
/// configuration is reported by the function surface
/// ([`ComAmazonawsLambdaAwsgirapiserviceState::code_signing_config_users`]) instead.
pub const CODE_SIGNING_CONFIG_IN_USE_MESSAGE: &str =
    "Lambda cannot delete the code signing configuration at this time — another operation \
     is in progress. Try again later.";

/// The refusal an operation ON a code-signing configuration answers when this account
/// holds none under the ARN it named: the sentence AWS words that case as, with the ARN the
/// request carried as its variable part.
///
/// A code-signing configuration is one of the resource kinds AWS gives its own 404 sentence
/// to rather than the canonical [`RESOURCE_NOT_FOUND_MESSAGE`], so every operation whose
/// OWN resource is the configuration — `GetCodeSigningConfig`, `UpdateCodeSigningConfig`,
/// `DeleteCodeSigningConfig` and `ListFunctionsByCodeSigningConfig`, each addressing it by
/// `CodeSigningConfigArn` — answers this one sentence, and a configuration that was
/// deleted reads exactly like one that never existed.
///
/// It is a different sentence from [`code_signing_config_not_found_message`], which belongs
/// to the modelled `CodeSigningConfigNotFoundException` an operation on a FUNCTION answers
/// when the configuration it was told to associate is not there.
#[must_use]
pub fn code_signing_config_gone_message(arn: &str) -> String {
    format!("The Lambda code signing configuration {arn} can not be found.")
}

/// The most published versions this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`], and bounding
/// the same way: every `PublishVersion` keeps a whole configuration snapshot in memory,
/// including for a package whose bytes this emulator holds none of (so one that counts
/// zero against the account's code-storage quota), and past this point the slice answers
/// the modelled `CodeStorageExceededException` rather than growing without bound. AWS
/// bounds a function's versions by that code-storage quota instead, and answers the same
/// modelled error.
pub const MAX_FUNCTION_VERSIONS: usize = 500;

/// `AccountLimit$TotalCodeSize` — "The amount of storage space that you can use for all
/// deployment packages and layer archives", in bytes.
///
/// AWS publishes this quota as 75 GB per account per Region ("Function and layer
/// storage", Lambda quotas), which is 75 * 1024^3 bytes. It is adjustable on a real
/// account; this emulator models neither Service Quotas nor an account's adjustments, so
/// it reports the default.
pub const TOTAL_CODE_SIZE_LIMIT: i64 = 80_530_636_800;

/// `AccountLimit$CodeSizeUnzipped` — "The maximum size of a function's deployment
/// package and layers when they're extracted", in bytes.
///
/// AWS publishes 250 MB (Lambda quotas, "Deployment package (.zip file archive) size …
/// 250 MB (unzipped)"), which is 250 * 1024^2 bytes. Not adjustable.
pub const CODE_SIZE_UNZIPPED_LIMIT: i64 = 262_144_000;

/// `AccountLimit$CodeSizeZipped` — "The maximum size of a deployment package when it's
/// uploaded directly to Lambda", in bytes.
///
/// AWS publishes 50 MB for a direct upload (Lambda quotas, "50 MB (zipped, for direct
/// upload)"), which is 50 * 1024^2 bytes. Not adjustable.
pub const CODE_SIZE_ZIPPED_LIMIT: i64 = 52_428_800;

/// `AccountLimit$ConcurrentExecutions` — "The maximum number of simultaneous function
/// executions".
///
/// AWS publishes a default of 1,000 per Region (Lambda quotas, "Concurrent executions"),
/// adjustable on a real account; this emulator reports the default for the same reason
/// as [`TOTAL_CODE_SIZE_LIMIT`].
pub const CONCURRENT_EXECUTIONS_LIMIT: i32 = 1_000;

/// The runtime identifiers Lambda no longer supports.
///
/// The model enumerates every runtime identifier the API accepts (`Runtime`) and says
/// nothing about which of them are past their deprecation date, so the set is a CATALOG:
/// the identifiers listed as deprecated in the AWS Lambda Developer Guide's "Runtime
/// deprecation policy" (its "Deprecated runtimes" table), each of whose deprecation dates
/// has passed. Every other member of `Runtime` is a supported runtime.
pub const DEPRECATED_RUNTIMES: &[&str] = &[
    "dotnetcore1.0",
    "dotnetcore2.0",
    "dotnetcore2.1",
    "dotnetcore3.1",
    "go1.x",
    "java8",
    "nodejs",
    "nodejs10.x",
    "nodejs12.x",
    "nodejs14.x",
    "nodejs16.x",
    "nodejs18.x",
    "nodejs4.3",
    "nodejs4.3-edge",
    "nodejs6.10",
    "nodejs8.10",
    "provided",
    "python2.7",
    "python3.6",
    "python3.7",
    "python3.8",
    "ruby2.5",
    "ruby2.7",
];

/// Whether `runtime` is one of the identifiers [`DEPRECATED_RUNTIMES`] holds.
///
/// A runtime identifier is a modelled `Runtime` enum value, matched exactly: the
/// identifiers differ only in their version suffixes, so a prefix or case-folded match
/// would read one runtime as another.
#[must_use]
pub fn is_deprecated_runtime(runtime: &str) -> bool {
    DEPRECATED_RUNTIMES.contains(&runtime)
}

/// The simultaneous executions an account must leave unreserved.
///
/// `PutFunctionConcurrency`'s own documentation states the rule: "You can reserve
/// concurrency for as many functions as you like, as long as you leave at least 100
/// simultaneous executions unreserved for functions that aren't configured with a
/// per-function limit." So a reservation that would push
/// `AccountLimit$UnreservedConcurrentExecutions` below this floor is refused.
pub const MIN_UNRESERVED_CONCURRENT_EXECUTIONS: i32 = 100;

/// Lambda's sentence for a write that would leave the account's
/// `AccountLimit$UnreservedConcurrentExecutions` below
/// [`MIN_UNRESERVED_CONCURRENT_EXECUTIONS`].
///
/// A COMPLETE sentence: every word outside the brackets is fixed, the terminating period
/// included, and the only value interpolated is the floor the account's limits hold — so
/// nothing of the request appears in it. `specified` is how the sentence names the
/// capacity the request asked to commit (the reservation a `PutFunctionConcurrency`
/// requests, the concurrency a provisioning call allocates); every operation that
/// enforces the floor states it through here so one wording answers for the one rule.
#[must_use]
pub fn unreserved_floor_message(specified: &str, floor: i32) -> String {
    format!(
        "Specified {specified} for function decreases account's UnreservedConcurrentExecution \
         below its minimum value of [{floor}]."
    )
}

/// Lambda's sentence for an allocation of provisioned concurrency larger than the
/// capacity the account has left UNRESERVED.
///
/// A DIFFERENT rule from [`unreserved_floor_message`], with a different remedy: this one
/// says the pool the allocation is drawn from does not hold what was asked for at all,
/// while the floor rule says the pool holds it but may not be emptied that far. The only
/// value interpolated is the account's unreserved concurrency — the quantity
/// `AccountLimit$UnreservedConcurrentExecutions` reports — so nothing of the request
/// appears in it.
#[must_use]
pub fn provisioned_above_unreserved_message(unreserved: i32) -> String {
    format!(
        "Specified ConcurrentExecutions for function is greater than account's unreserved \
         concurrency [{unreserved}]."
    )
}

/// Lambda's sentence for an allocation of provisioned concurrency larger than the
/// concurrency the FUNCTION reserves.
///
/// A function that reserves concurrency allocates its provisioned concurrency out of that
/// reservation, so the two are weighed against each other per function rather than
/// against the account — a different rule again from
/// [`provisioned_above_unreserved_message`] and [`unreserved_floor_message`], and the one
/// signal a caller has for which limit to raise. That one rule is violable from either
/// side, and each side states what IT asked for: this sentence answers the call that
/// ALLOCATES provisioned concurrency past the reservation, so it names the provisioned
/// request and is a CONSTANT with no slot; the call that LOWERS the reservation below
/// what is already provisioned is answered by [`reserved_below_provisioned_message`]
/// instead.
pub const PROVISIONED_ABOVE_RESERVED_MESSAGE: &str =
    "Requested Provisioned Concurrency should not be greater than the \
     reservedConcurrentExecution for function";

/// Lambda's sentence for a reservation lower than the provisioned concurrency the
/// FUNCTION has already committed across its versions and aliases.
///
/// The other side of the rule [`PROVISIONED_ABOVE_RESERVED_MESSAGE`] states: the capacity
/// a function keeps initialised is drawn from its reservation, so a reservation may not be
/// set below that total. The remedy differs from the allocating side's — release
/// provisioned concurrency first, rather than raise the reservation — so the sentence
/// names what the caller actually asked to do and both values it is weighed against:
/// `requested` is the reservation the request carries and `provisioned` the function's
/// total provisioned concurrency read from its record, so a caller can compute the
/// smallest reservation that would be admitted without reading the function back.
/// `specified` is how the sentence names the member the request carried the reservation
/// in.
#[must_use]
pub fn reserved_below_provisioned_message(
    specified: &str,
    requested: i32,
    provisioned: i32,
) -> String {
    format!(
        " {specified}  {requested} should not be lower than function's total provisioned \
         concurrency [{provisioned}]."
    )
}

/// The IAM policy language version a resource-based policy document carries.
/// `2012-10-17` is the current version of the language every AWS policy document is
/// written in.
pub const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The `Id` of the resource-based policy Lambda maintains for a function.
pub const POLICY_ID: &str = "default";

/// The `Effect` of every statement `AddPermission` writes: the operation "Grants a
/// principal permission to use a function", so the statement it adds is a grant and
/// never a denial.
pub const STATEMENT_EFFECT_ALLOW: &str = "Allow";

/// The largest a function's resource-based policy may get, in bytes.
///
/// AWS publishes the quota as 20 KB (Lambda quotas, "Function resource-based policy"),
/// and answers `PolicyLengthExceededException` ("The permissions policy for the resource
/// is too large") for a call that would push the policy past it. Not adjustable.
pub const MAX_POLICY_SIZE_BYTES: usize = 20_480;

/// The refusal a grant that would push a resource policy past [`MAX_POLICY_SIZE_BYTES`]
/// gets: the fixed sentence `PolicyLengthExceededException` carries, whose variables are
/// the size the policy WOULD have reached and the quota it may not pass.
///
/// It states the outcome of the call rather than the resource it was made on — the request
/// already names that — so one sentence serves every policy this service holds, and a
/// caller can read off how much too large the result was.
#[must_use]
pub fn policy_length_exceeded_message(final_size: usize) -> String {
    format!(
        "The final policy size ({final_size}) is bigger than the limit \
         ({MAX_POLICY_SIZE_BYTES})."
    )
}

/// The message every resource-policy operation states when the ARN it was given names
/// no policy: a resource of a kind this store holds no records of, or one nobody has
/// attached a policy to. It states the fact and nothing from the request, so the two
/// cases read alike. Source for the wording: the answer the corpus unit
/// `lambda.resource-policy-lifecycle` carries for a `GetResourcePolicy` after the policy
/// was deleted.
pub const NO_RESOURCE_POLICY_MESSAGE: &str = "The resource you requested does not exist.";

/// The message `PutResourcePolicy` and `DeleteResourcePolicy` both state when the
/// `RevisionId` they were given is not the stored policy's.
///
/// The condition carries nothing from the request — the id given is simply not the
/// current one — and the sentence points the caller at where the current one comes from,
/// which is what both members document ("To retrieve the current revision ID, use the
/// GetResourcePolicy operation"). Source for the wording: the answers the corpus units
/// `lambda.resource-policy-revisionid-precondition` and
/// `lambda.resource-policy-stale-revision-delete-rejected` carry.
pub const RESOURCE_POLICY_REVISION_MISMATCH_MESSAGE: &str =
    "The value of RevisionId provided doesn't match the policy's latest revision ID. Use the \
     GetResourcePolicy API action to retrieve the current revision ID and try again.";

/// The service principal a function's execution role has to let assume it.
///
/// "The execution role grants the function permission to use Amazon Web Services
/// services": the function RUNS AS that role, and it is this service that assumes it on
/// the function's behalf, so the role's trust policy has to name this principal.
pub const EXECUTION_ROLE_PRINCIPAL: &str = "lambda.amazonaws.com";

/// Why a request naming an execution role this service cannot assume is refused.
pub const ROLE_NOT_ASSUMABLE_MESSAGE: &str =
    "The role defined for the function cannot be assumed by Lambda.";

/// Whether IAM ANSWERED that `role_arn` is an execution role this service could not
/// assume: this account's IAM HOLDS a role under the name the ARN carries, and the trust
/// policy that stored role carries does not let [`EXECUTION_ROLE_PRINCIPAL`] assume it.
///
/// A function RUNS AS the role the caller names, so every operation that STORES a
/// function's execution role resolves it before it writes, and a request naming one this
/// service cannot assume is refused with the operation's modelled 400 with nothing
/// persisted. IAM is reached the way any emulated service reaches another: through the
/// in-process callback ([`smithy_server_runtime::OperationContext::callback`]), so this
/// crate names no other service crate.
///
/// `false` — do not refuse — for every case where nothing is KNOWN about the role: a value
/// that names no IAM role of this account (another account's IAM is not this emulator's to
/// ask), an emulator with no IAM mounted to ask at all, and a name this account's IAM holds
/// no role under. That last case is what a function CREATED WITH A ROLE PROVISIONED
/// ELSEWHERE looks like here: an execution role is normally created outside the client that
/// creates the function, so this emulator's IAM holding no record of it says nothing about
/// the role itself, and the create is served. A refusal has to be grounded in a trust
/// document IAM answered with, never assumed from an absent record.
/// The whole question — which role of this account the value names, what IAM answers
/// about it, and whether the trust policy it carries admits a service principal — is
/// the ONE reading every service that resolves a caller-supplied execution role shares
/// (`service_common::iam_role`), so this crate states only WHICH principal has to be
/// admitted, WHICH operations ask, and that a role no record is held for is served.
pub async fn execution_role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    ::service_common::iam_role::held_role_is_unassumable(
        ctx,
        region,
        role_arn,
        EXECUTION_ROLE_PRINCIPAL,
    )
    .await
}

/// One layer a function's execution environment holds.
///
/// `FunctionConfiguration$Layers` reports "The function's layers" as the layer version's
/// ARN and "The size of the layer archive in bytes", so the resolution of a `Layers`
/// entry against the published layer version is what is stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionLayer {
    /// The ARN of the layer version, including its version number.
    pub arn: String,
    /// The size of that version's archive, in bytes.
    pub code_size: i64,
    /// The size of that version's archive once expanded, in bytes — the quantity the
    /// unzipped-size limit of [`CODE_SIZE_UNZIPPED_LIMIT`] is measured against, measured
    /// by [`unzipped_bytes`] off the stored archive when the version was attached. A
    /// published layer version is immutable, so the number cannot go stale.
    pub uncompressed_code_size: i64,
    /// The archive's own bytes, when this emulator holds them — the CONTENT a managed
    /// runtime mounts under the layer root, which is what a handler importing from the
    /// layer loads. Held with the resolution for the same reason the sizes are: a
    /// published layer version is immutable, so the bytes cannot go stale. SHARED rather
    /// than copied, so attaching a layer to a function and invoking it hold one copy of
    /// the archive between them.
    pub zip_bytes: Option<::std::sync::Arc<Vec<u8>>>,
}

/// The configuration of one function version — the members `FunctionConfiguration`
/// reports that a published version freezes.
///
/// Held apart from [`FunctionRecord`] so publishing a version is a snapshot of exactly
/// these fields and nothing else.
#[derive(Debug, Clone)]
pub struct FunctionConfig {
    /// The function's name (never an ARN or a qualified name).
    pub name: String,
    /// The execution role ARN, as supplied.
    pub role: String,
    /// The runtime identifier, absent for an `Image` package.
    pub runtime: Option<String>,
    /// The handler, absent for an `Image` package.
    pub handler: Option<String>,
    /// The description, when the caller gave one.
    pub description: Option<String>,
    /// The timeout, in seconds.
    pub timeout: i32,
    /// The memory size, in MB.
    pub memory_size: i32,
    /// `Zip` or `Image`.
    pub package_type: String,
    /// Exactly one architecture, per the model's `@length(min: 1, max: 1)`.
    pub architectures: Vec<String>,
    /// The environment variables, ordered so a read reports them deterministically.
    pub environment: BTreeMap<String, String>,
    /// The layer versions this configuration holds, in the order the caller named
    /// them, each resolved against the layer the store holds.
    pub layers: Vec<FunctionLayer>,
    /// The `/tmp` size, in MB.
    pub ephemeral_storage_mb: i32,
    /// The X-Ray tracing mode.
    pub tracing_mode: String,
    /// The KMS key ARN, when the caller named one.
    pub kms_key_arn: Option<String>,
    /// The size of the stored deployment package, in bytes; 0 when this emulator holds
    /// no bytes for it (an S3 object or a container image it does not resolve).
    pub code_size: i64,
    /// The base64 SHA-256 of the stored deployment package bytes; absent when this
    /// emulator holds no bytes to hash.
    pub code_sha256: Option<String>,
    /// ISO-8601 instant of the last change to this configuration.
    pub last_modified: String,
    /// The revision this configuration is at.
    pub revision_id: String,
    /// The VPC the function is attached to, when the caller named one.
    pub vpc_config: Option<VpcConfig>,
    /// The dead-letter queue for failed asynchronous invocations, when the caller named
    /// one.
    pub dead_letter_config: Option<DeadLetterConfig>,
    /// The file systems the function mounts, in the order the caller named them (the
    /// model's `FileSystemConfigList` holds at most one).
    pub file_system_configs: Vec<FileSystemConfig>,
    /// The container image overrides, when the caller named any.
    pub image_config: Option<ImageConfig>,
    /// `SnapStart$ApplyOn`, when the caller set it.
    pub snap_start_apply_on: Option<String>,
    /// The CloudWatch Logs configuration, when the caller named one.
    pub logging_config: Option<LoggingConfig>,
    /// The capacity provider that manages this function's compute, when the caller
    /// named one.
    pub capacity_provider_config: Option<CapacityProviderConfig>,
    /// The durable-execution settings, when the caller named them.
    pub durable_config: Option<DurableConfig>,
    /// The tenant isolation settings, when the caller named them. `CreateFunction` is
    /// the only operation that sets this one: "Tenant isolation configuration cannot be
    /// modified after function creation", and `UpdateFunctionConfiguration` models no
    /// member for it.
    pub tenancy_config: Option<TenancyConfig>,
}

/// One published version of a function: its frozen configuration plus when it was
/// published.
#[derive(Debug, Clone)]
pub struct FunctionVersionRecord {
    /// The numeric version label ("1", "2", ...).
    pub version: u64,
    /// The configuration this version froze, carrying the version's OWN
    /// [`FunctionConfig::revision_id`] (see [`FunctionRecord::publish_next_version`]).
    pub config: FunctionConfig,
    /// The revision the unpublished configuration was at when this version was
    /// published.
    ///
    /// A published version is a distinct identity with a revision of its own, so the
    /// revision it was taken FROM is the only thing that can say whether the unpublished
    /// configuration has changed since — which is the question
    /// "Lambda doesn't publish a version if the function's configuration and code haven't
    /// changed since the last version" asks.
    pub source_revision_id: String,
    /// The deployment package this version froze.
    ///
    /// A version keeps its own copy of the package: `PublishVersion` "Creates a version
    /// from the current code and configuration of a function", and a later
    /// `UpdateFunctionCode` changes the unpublished version only — so the bytes a
    /// published version's `Code$Location` serves stay the ones it was published from.
    pub code: CodeRecord,
    /// ISO-8601 instant the version was published.
    pub created: String,
}

/// `value` as a JSON string literal, escaped by the JSON serializer rather than by
/// hand.
fn json_string(value: &str) -> String {
    ::serde_json::Value::String(value.to_string()).to_string()
}

/// A JSON object whose members keep the order they are given in.
///
/// A `serde_json::Value` map would sort them by key, and a policy document reads in a
/// fixed order (`Sid`, `Effect`, `Principal`, `Action`, `Resource`, `Condition`), so the
/// members are rendered in order while every VALUE is still serialized by `serde_json`.
fn json_object(members: &[(&str, String)]) -> String {
    let rendered: Vec<String> = members
        .iter()
        .map(|(name, value)| format!("{}:{value}", json_string(name)))
        .collect();
    format!("{{{}}}", rendered.join(","))
}

/// WHICH identities a grant may name, and the refusal a value naming none of them gets;
/// see the module's own header.
pub mod policy_principal;
pub use policy_principal::{
    is_account_id, is_grantable_principal, is_service_principal, EVERY_PRINCIPAL,
    INVALID_PRINCIPAL_MESSAGE,
};

/// The `Principal` of a policy statement for the principal `AddPermission` was given.
///
/// The model documents the forms the member takes — "The Amazon Web Services service,
/// Amazon Web Services account, IAM user, or IAM role that invokes the function", where
/// "For Amazon Web Services services, the principal is a domain-style identifier that
/// the service defines, such as s3.amazonaws.com" — and the policy language spells each
/// of them differently:
///
/// * an account id is that account's root principal, written as its ARN;
/// * an IAM user or role is already an ARN, and is used as given;
/// * anything else is the domain-style identifier of a service principal;
/// * `*` is every principal, which the language writes as the bare string.
///
/// Which of those forms a value IS is read by [`policy_principal`], the same reading that
/// decides whether a grant may name it at all, so what is admitted and what is written
/// here cannot disagree.
#[must_use]
pub fn principal_json(principal: &str) -> String {
    if principal == EVERY_PRINCIPAL {
        return json_string(principal);
    }
    if is_account_id(principal) {
        return json_object(&[(
            "AWS",
            json_string(&format!("arn:aws:iam::{principal}:root")),
        )]);
    }
    if principal.starts_with("arn:") {
        return json_object(&[("AWS", json_string(principal))]);
    }
    json_object(&[("Service", json_string(principal))])
}

/// One statement of a function's resource-based policy: exactly the grant one
/// `AddPermission` call described, plus the ARN it was granted on.
#[derive(Debug, Clone)]
pub struct PolicyStatement {
    /// The `StatementId`, which "differentiates the statement from others in the same
    /// policy" and is the statement's `Sid`.
    pub sid: String,
    /// The action the principal may use on the function.
    pub action: String,
    /// The principal, as supplied; [`principal_json`] renders it.
    pub principal: String,
    /// The ARN the permission is granted on: the function's unqualified ARN for a
    /// function-level statement, and the qualified one for a version or alias.
    pub resource: String,
    /// `SourceArn`, when the caller limited the grant to one invoking resource.
    pub source_arn: Option<String>,
    /// `SourceAccount`, when the caller limited the grant to one owning account.
    pub source_account: Option<String>,
    /// `PrincipalOrgID`, when the caller limited the grant to one organization.
    pub principal_org_id: Option<String>,
    /// `EventSourceToken`, when the caller required one.
    pub event_source_token: Option<String>,
    /// `FunctionUrlAuthType`, when the grant is about a function URL.
    pub function_url_auth_type: Option<String>,
    /// `InvokedViaFunctionUrl`, when the caller restricted the grant to invocations
    /// that come through a function URL.
    pub invoked_via_function_url: Option<bool>,
}

impl PolicyStatement {
    /// The `Condition` block of this statement, or `None` when the grant carries no
    /// restriction at all.
    ///
    /// Each optional member of the request restricts the grant, and each is written as
    /// the IAM condition key that expresses it. `SourceArn` is compared as an ARN — the
    /// model's note "Lambda configures the comparison using the `StringLike` operator"
    /// is why wildcards in the ARN match, which is what the ARN-typed `ArnLike` operator
    /// does — the remaining string-valued keys are compared for equality, and the
    /// boolean one with `Bool`.
    fn condition_json(&self) -> Option<String> {
        let mut operators: Vec<(&str, String)> = Vec::new();
        if let Some(source_arn) = &self.source_arn {
            operators.push((
                "ArnLike",
                json_object(&[("AWS:SourceArn", json_string(source_arn))]),
            ));
        }

        let mut equals: Vec<(&str, String)> = Vec::new();
        if let Some(source_account) = &self.source_account {
            equals.push(("AWS:SourceAccount", json_string(source_account)));
        }
        if let Some(org_id) = &self.principal_org_id {
            equals.push(("aws:PrincipalOrgID", json_string(org_id)));
        }
        if let Some(token) = &self.event_source_token {
            equals.push(("lambda:EventSourceToken", json_string(token)));
        }
        if let Some(auth_type) = &self.function_url_auth_type {
            equals.push(("lambda:FunctionUrlAuthType", json_string(auth_type)));
        }
        if !equals.is_empty() {
            operators.push(("StringEquals", json_object(&equals)));
        }

        if let Some(via_function_url) = self.invoked_via_function_url {
            operators.push((
                "Bool",
                json_object(&[(
                    "lambda:InvokedViaFunctionUrl",
                    json_string(&via_function_url.to_string()),
                )]),
            ));
        }

        (!operators.is_empty()).then(|| json_object(&operators))
    }

    /// This statement as the JSON `AddPermission` answers with and `GetPolicy` embeds:
    /// "The permission statement that's added to the function policy".
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut members = vec![
            ("Sid", json_string(&self.sid)),
            ("Effect", json_string(STATEMENT_EFFECT_ALLOW)),
            ("Principal", principal_json(&self.principal)),
            ("Action", json_string(&self.action)),
            ("Resource", json_string(&self.resource)),
        ];
        if let Some(condition) = self.condition_json() {
            members.push(("Condition", condition));
        }
        json_object(&members)
    }
}

/// The member of a policy document that carries its statements, as the IAM policy
/// language names it.
const POLICY_STATEMENT_MEMBER: &str = "Statement";

/// The member of a policy statement that carries its id, as the IAM policy language
/// names it; it is the `StatementId` an `AddPermission` grant was made under.
const STATEMENT_ID_MEMBER: &str = "Sid";

/// The refusal a value that is not a policy document gets, on the operations that take one
/// as a string member.
///
/// The `Policy` member is "formatted as a JSON string", and what is wrong with a value that
/// is not one is that it does not PARSE as the policy language's document: the sentence
/// states that and nothing of the request, so it reads the same whatever was submitted.
pub const POLICY_SYNTAX_ERROR_MESSAGE: &str = "Policy has syntax errors.";

/// Whether `document` is a policy document.
///
/// The `ResourcePolicy` member is "formatted as a JSON string", and the policy language
/// writes a policy as a JSON OBJECT (a `Version` and the statements under
/// [`POLICY_STATEMENT_MEMBER`]), so a value that is not one is no policy at all.
#[must_use]
pub fn is_policy_document(document: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(document).is_ok_and(|parsed| parsed.is_object())
}

/// The `Sid` of the rendered policy statement `statement`, when it carries one.
fn statement_id_of(statement: &::serde_json::Value) -> Option<&str> {
    statement
        .get(STATEMENT_ID_MEMBER)
        .and_then(::serde_json::Value::as_str)
}

/// `document` with `statement` — one rendered policy statement — appended to the
/// statements it carries, or `None` when the two cannot be merged at all.
///
/// A document this store holds was accepted as a JSON object and a statement is rendered
/// here, so both parse and the merge is always made; `None` is the answer for a value
/// that is somehow neither, which lets the caller keep the statement rather than report a
/// grant it did not store (see [`PolicyRecord::add_statement`]).
///
/// The merged document is RE-SERIALIZED, so a document that carried its members in
/// another order, or any whitespace, is reported in this one afterwards: the policy is
/// the same policy, but only a document nobody has added a statement to since is reported
/// byte for byte as it was supplied.
fn document_with_statement(document: &str, statement: &str) -> Option<String> {
    let (Ok(mut parsed), Ok(added)) = (
        ::serde_json::from_str::<::serde_json::Value>(document),
        ::serde_json::from_str::<::serde_json::Value>(statement),
    ) else {
        return None;
    };
    let members = parsed.as_object_mut()?;
    let held = members
        .entry(POLICY_STATEMENT_MEMBER)
        .or_insert_with(|| ::serde_json::Value::Array(Vec::new()));
    match held {
        // The language admits a single statement as well as a list of them, so a
        // document carrying one becomes a document carrying both.
        ::serde_json::Value::Array(statements) => statements.push(added),
        single => {
            let first = single.take();
            *single = ::serde_json::Value::Array(vec![first, added]);
        }
    }
    Some(parsed.to_string())
}

/// The resource-based policy of one function, version or alias: the statements
/// `AddPermission` has added to it, or the document a `PutResourcePolicy` stored, and
/// the revision they are at.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    /// The statements, in the order they were added, so the document reports them
    /// deterministically. Empty for a policy held as a whole [`PolicyRecord::document`].
    pub statements: Vec<PolicyStatement>,
    /// The whole policy document a caller supplied, in the form this service reports it.
    ///
    /// `PutResourcePolicy` "replaces any existing policy on the Lambda resource. If you
    /// previously added permissions using the AddPermission operation, the new policy
    /// overwrites those permissions", so a policy stored that way IS that document: the
    /// statements above are then empty and a later grant is merged into it (see
    /// [`PolicyRecord::add_statement`]). `None` for a policy made up of statements.
    pub document: Option<String>,
    /// The revision this policy is at. `GetPolicy` reports it, and `AddPermission`'s
    /// `RevisionId` precondition is checked against it ("For AddPermission and
    /// RemovePermission API operations: Call GetPolicy to retrieve the latest RevisionId
    /// for your resource"); a fresh one is minted on every change.
    pub revision_id: String,
}

impl Default for PolicyRecord {
    fn default() -> Self {
        Self {
            statements: Vec::new(),
            document: None,
            revision_id: mint_revision_id(),
        }
    }
}

impl PolicyRecord {
    /// The policy a caller supplied as a whole document, at a fresh revision.
    ///
    /// The document is STORED as this service reports it, which is not the caller's bytes:
    /// a resource policy is reported in the one rendering every service that attaches an
    /// IAM policy document reports it in (`service_common::iam_policy_rendering`) — the
    /// insignificant whitespace between its JSON tokens dropped, and an element the policy
    /// language types as a string or an array of them written as the string when the caller
    /// wrote a one-element array. Everything else is the caller's: member order, the
    /// spelling of every value, and every byte inside a string literal. So the policy a
    /// caller reads back is the same text whatever layout it was submitted in, and putting
    /// back what was read stores the same bytes again.
    #[must_use]
    pub fn of_document(document: String) -> Self {
        Self {
            statements: Vec::new(),
            document: Some(::service_common::iam_policy_rendering::rendered(&document)),
            revision_id: mint_revision_id(),
        }
    }

    /// Whether this policy already holds a statement with `sid` — among the statements
    /// `AddPermission` added, or among those the stored document carries.
    #[must_use]
    pub fn holds(&self, sid: &str) -> bool {
        if self.statements.iter().any(|statement| statement.sid == sid) {
            return true;
        }
        let Some(document) = &self.document else {
            return false;
        };
        let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
            return false;
        };
        match parsed.get(POLICY_STATEMENT_MEMBER) {
            Some(::serde_json::Value::Array(statements)) => statements
                .iter()
                .any(|statement| statement_id_of(statement) == Some(sid)),
            Some(single) => statement_id_of(single) == Some(sid),
            None => false,
        }
    }

    /// Add `statement` to this policy, which is what `AddPermission` appends to it.
    ///
    /// A policy held as a caller-supplied document keeps being that document, so the
    /// statement joins the statements that document carries; otherwise it joins the
    /// statements this record holds.
    ///
    /// A statement merged into a document is part of the document from then on, so
    /// `RemovePermission` — which retains over the statements this record holds
    /// structured — cannot take it back out again, and refuses with the statement id it
    /// cannot find rather than reporting a removal it did not make. Removing from a
    /// stored document is the remaining half of that operation's own slice.
    pub fn add_statement(&mut self, statement: PolicyStatement) {
        // The document is taken out first, so a value that cannot carry the statement at
        // all (one that is somehow not the JSON object a stored document was accepted as)
        // leaves this record holding the statement structured rather than reporting a
        // grant it stored nowhere.
        match self
            .document
            .take()
            .and_then(|document| document_with_statement(&document, &statement.to_json()))
        {
            Some(merged) => self.document = Some(merged),
            None => self.statements.push(statement),
        }
    }

    /// The whole policy document, which is what `GetPolicy`'s and
    /// `GetResourcePolicy`'s `Policy` member carries.
    #[must_use]
    pub fn to_json(&self) -> String {
        if let Some(document) = &self.document {
            return document.clone();
        }
        let statements: Vec<String> = self
            .statements
            .iter()
            .map(PolicyStatement::to_json)
            .collect();
        json_object(&[
            ("Version", json_string(POLICY_LANGUAGE_VERSION)),
            ("Id", json_string(POLICY_ID)),
            (
                POLICY_STATEMENT_MEMBER,
                format!("[{}]", statements.join(",")),
            ),
        ])
    }
}

/// The `InvokeMode` a function URL SERVES IN when no caller ever configured one, which
/// the model's own member documentation states: "BUFFERED – This is the default option.
/// Lambda invokes your function using the Invoke API operation."
///
/// It is the behaviour of an unconfigured URL, not a value stored on it: see
/// [`FunctionUrlConfigRecord::invoke_mode`].
pub const DEFAULT_INVOKE_MODE: &str = "BUFFERED";

/// The host suffix every Lambda function URL carries. AWS documents the form in
/// `CreateFunctionUrlConfig`'s guide ("Function URL endpoints have the following
/// format: `https://<url-id>.lambda-url.<region>.on.aws`"), and it is what keeps the
/// minted URL inside the model's 40..100 `FunctionUrl` length constraint.
pub const FUNCTION_URL_HOST_SUFFIX: &str = "lambda-url";

/// The URL of the function URL whose id is `url_id`, in the region it was created in, in
/// the form AWS documents.
///
/// The host is the one [`function_url::function_url_endpoint_host`] spells and the trailing
/// `/` is part of the endpoint AWS vends, so the value a caller gets back is the URL they
/// can address as-is.
///
/// What this service ANNOUNCES for a function URL is
/// [`function_url::announced_function_url`], which answers this form only for a caller that
/// resolves AWS's own hosts — every other caller is announced an address this emulator
/// itself answers on, carrying this same endpoint host.
#[must_use]
pub fn function_url(region: &str, url_id: &str) -> String {
    format!(
        "https://{}/",
        function_url::function_url_endpoint_host(region, url_id)
    )
}

/// A fresh function-URL id: the opaque, per-URL subdomain label AWS mints for a
/// function URL. AWS's are 32 lowercase alphanumeric characters, which the 32 hex
/// digits of a random UUID satisfy. Minted once, at creation, and stored — so every
/// later read of the same function URL reports the same URL.
#[must_use]
pub fn mint_function_url_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// The model's `FunctionUrlQualifier` pattern, which is the one every function-URL
/// operation puts on the qualifier it accepts.
pub const FUNCTION_URL_QUALIFIER_PATTERN: &str = r"^((?!^\d+$)^[0-9a-zA-Z-_]+$)$";

/// Whether `qualifier` is one a function URL may be addressed by.
///
/// A function URL belongs to a function's unpublished version — addressed by naming no
/// qualifier at all — or to one of its aliases, and the model says exactly that through
/// [`FUNCTION_URL_QUALIFIER_PATTERN`]: an all-digit value (a published version number)
/// does not match it, and neither does [`VERSION_LATEST`], whose `$` the pattern's
/// character class does not admit.
#[must_use]
pub fn is_function_url_qualifier(qualifier: &str) -> bool {
    !qualifier.is_empty()
        && qualifier != VERSION_LATEST
        && !qualifier.bytes().all(|byte| byte.is_ascii_digit())
}

/// The refusal for a qualifier a function URL cannot be addressed by: a validation
/// message built from the request, naming the member and the constraint it failed.
#[must_use]
pub fn function_url_qualifier_message(qualifier: &str) -> String {
    pattern_violation_message("qualifier", qualifier, FUNCTION_URL_QUALIFIER_PATTERN)
}

/// The refusal for a value that does not satisfy the `@pattern` its member carries: the
/// clause the framework's own constraint walk states, built from the request — the value,
/// the member path, and the expression the value has to satisfy.
///
/// The expression is stated the way the same refusal states it when the walk catches it
/// at the boundary: without the `^`/`$` a whole-value Smithy pattern is wrapped in
/// (`service_common::validation::unanchored`, which `service::override_refusal` already
/// routes every boundary clause through), so one request cannot be told the expression
/// two ways depending on which of the two checks refused it.
#[must_use]
pub fn pattern_violation_message(path: &str, value: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: {}",
        ::service_common::validation::unanchored(pattern)
    )
}

/// The model's `NumericLatestPublishedOrAliasQualifier` pattern, which every operation
/// addressing a function by a `Qualifier` puts on that member
/// (`smithy_query(query="shape", shape="NumericLatestPublishedOrAliasQualifier")`).
pub const QUALIFIER_PATTERN: &str = r"^\$(LATEST(\.PUBLISHED)?)|[a-zA-Z0-9-_$]+$";

/// Whether `qualifier` is a value the qualifier pattern admits as a WHOLE value: the
/// unpublished version ([`VERSION_LATEST`]) or its published reading
/// ([`VERSION_LATEST_PUBLISHED`]), or a run of the characters a published version number
/// and an alias name are written with.
///
/// A Smithy `@pattern` is a FIND rather than a total match, so the framework's own
/// constraint walk admits any value that merely CONTAINS what one branch of
/// [`QUALIFIER_PATTERN`]'s alternation matches — a qualifier carrying a space is admitted
/// on the run that follows the space. AWS validates the alternation against the whole
/// value, so this states that reading: a value the alternation does not match end to end
/// is a validation failure ([`qualifier_message`]) rather than a qualifier to look up.
#[must_use]
pub fn is_qualifier(qualifier: &str) -> bool {
    if qualifier == VERSION_LATEST || qualifier == VERSION_LATEST_PUBLISHED {
        return true;
    }
    !qualifier.is_empty()
        && qualifier.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'$'
        })
}

/// The refusal a `Qualifier` outside [`QUALIFIER_PATTERN`] gets: the validation clause
/// built from the request — the value, the member path and the expression the value has to
/// satisfy — which is what the framework's own walk would have stated had the pattern been
/// a whole-value match.
///
/// Every operation spells the member `Qualifier`, which a clause states as `qualifier`.
#[must_use]
pub fn qualifier_message(qualifier: &str) -> String {
    pattern_violation_message("qualifier", qualifier, QUALIFIER_PATTERN)
}

/// Refuse a `Qualifier` that [`QUALIFIER_PATTERN`] does not admit as a whole value, so an
/// operation addressing a function by one states the violation before it looks anything
/// up.
///
/// The refusal is the one the framework's own constraint walk answers — 400
/// `ValidationException` over the clause built from the request — because what is wrong is
/// the request's SHAPE rather than what the value means. That is the same answer, for the
/// same reason, that the alias-name pattern gets (`crate::ops::create_alias`): a pattern
/// the walk cannot decide is decided here and worded as the walk would have worded it.
///
/// Generic over the operation's modelled error, so every operation that declares a
/// `NumericLatestPublishedOrAliasQualifier` states this one way. An operation whose
/// `Qualifier` targets ANOTHER shape is not one of them — the clause quotes the expression
/// that member declares, and the function-URL and provisioned-concurrency operations each
/// declare their own ([`function_url_qualifier_message`] states one of them).
///
/// A qualifier the caller left EMPTY is not refused here: it is outside the member's
/// `@length` minimum, which the boundary walk already states.
pub fn admitted_qualifier<E>(
    qualifier: Option<&str>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    match qualifier.filter(|named| !named.is_empty()) {
        Some(named) if !is_qualifier(named) => Err(crate::scaffolded::Scaffolded::validation(
            qualifier_message(named),
        )),
        _ => Ok(()),
    }
}

/// Lambda's canonical `ResourceNotFoundException` sentence: the fixed wording the service
/// answers for a resource that does not exist, naming NOTHING of the request.
///
/// It is the whole message — no `Resource: <arn>` clause, no resource kind, no identifier
/// — because the request already names what it asked about, so the sentence is the same
/// one for every resource and a caller can compare it exactly. Only the resource kinds
/// AWS gives their OWN fixed sentence to depart from it: a function, version or alias the
/// request named ([`resolve_function`]'s `Function not found: <arn>`), an alias looked up
/// by name ([`alias_not_found_message`]), a layer version addressed by the operations on
/// its POLICY ([`resolve_layer_version`], where the version is the operation's own
/// resource), a code signing configuration an operation on the configuration itself named
/// ([`code_signing_config_gone_message`]) and the configuration-shaped refusals below.
/// Every other 404 in this service — a function URL, a layer version READ, an event source
/// mapping, a resource addressed by its TAGS, a function carrying no resource-based policy,
/// and the function-scaling operations, which word both the name and the qualifier they
/// could not resolve with this sentence — answers this. The READ of a function's policy
/// answers it for the resource as well: `GetPolicy` is asked for a document, so a pair that
/// resolves to nothing and a resource holding no policy are one answer there.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The resource you requested does not exist.";

/// The refusal a `RevisionId` precondition that did not hold gets on `AddPermission` and
/// `RemovePermission`.
///
/// `PreconditionFailedException` documents the whole rule
/// (`smithy_query(query="doc", shape="PreconditionFailedException")`): the revision
/// supplied is not the latest one, and the caller is told which API reports the latest —
/// "For AddPermission and RemovePermission API operations: Call `GetPolicy`"; "For all
/// other API operations: Call `GetFunction` or `GetAlias`". So there are exactly TWO fixed
/// sentences, one per branch of that rule, and neither carries anything of the request:
/// not the id the caller supplied, not the resource it addressed. This is the first
/// branch; [`CONFIGURATION_REVISION_PRECONDITION_MESSAGE`] is the second.
pub const POLICY_REVISION_PRECONDITION_MESSAGE: &str =
    "The Revision Id provided does not match the latest Revision Id. Call the GetPolicy \
     API to retrieve the latest Revision Id";

/// The second branch of the rule [`POLICY_REVISION_PRECONDITION_MESSAGE`] states: the
/// refusal every OTHER operation taking a `RevisionId` precondition gets, naming the two
/// APIs that report the latest revision of a function version and of an alias.
pub const CONFIGURATION_REVISION_PRECONDITION_MESSAGE: &str =
    "The Revision Id provided does not match the latest Revision Id. Call the \
     GetFunction/GetAlias API to retrieve the latest Revision Id";

/// The same refusal on the operations whose resource is a LAYER VERSION
/// (`AddLayerVersionPermission`, `RemoveLayerVersionPermission`).
///
/// The rule `PreconditionFailedException` documents is "call the API that reports the
/// latest revision of the resource you addressed", and for a layer version's policy that
/// API is `GetLayerVersion` rather than `GetFunction`/`GetAlias` — so the sentence names
/// the resource it is about and that API, and carries nothing of the request.
pub const LAYER_VERSION_REVISION_PRECONDITION_MESSAGE: &str =
    "The Revision Id provided does not match the latest Revision Id for the layer version. \
     Call the GetLayerVersion API to retrieve the latest Revision Id";

/// The qualifier a `FunctionName`/`Qualifier` pair NAMES, as opposed to the
/// [`VERSION_LATEST`] an unqualified request resolves to. It may come from either, and
/// [`resolve_function`] refuses a pair that names two different ones.
#[must_use]
pub fn named_qualifier(function_name: &str, qualifier: Option<&str>) -> Option<String> {
    qualifier
        .map(str::to_string)
        .or_else(|| parse_function_reference(function_name).and_then(|reference| reference.qualifier))
        .filter(|named| !named.is_empty())
}

/// One Lambda function URL: "a dedicated HTTP(S) endpoint that you can use to invoke
/// your function", with the configuration the caller gave it.
///
/// Held per qualifier on [`FunctionRecord::url_configs`], because a function URL is
/// attached to the function's unpublished version or to one of its aliases, and each of
/// those has at most one.
#[derive(Debug, Clone)]
pub struct FunctionUrlConfigRecord {
    /// The subdomain label the URL is addressed by, minted once by
    /// [`mint_function_url_id`].
    pub url_id: String,
    /// The HTTP(S) endpoint this function URL is ANNOUNCED on, resolved once at creation
    /// by [`function_url::announced_function_url`] — an address this emulator itself
    /// answers on — and stored, so `CreateFunctionUrlConfig`, `GetFunctionUrlConfig`,
    /// `UpdateFunctionUrlConfig` and `ListFunctionUrlConfigs` cannot report the same
    /// function URL at two different addresses.
    pub url: String,
    /// The `FunctionUrlAuthType` the URL enforces (`AWS_IAM` or `NONE`), as supplied.
    pub auth_type: String,
    /// The CORS settings, exactly as configured; absent when the caller configured
    /// none, because nothing here invents settings the caller never asked for.
    pub cors: Option<Cors>,
    /// The `InvokeMode` a caller CONFIGURED, and absent for a function URL no caller ever
    /// named one on — which is a different thing from the mode such a URL serves in
    /// ([`Self::served_invoke_mode`]): the member is optional, so a request that named
    /// none configured none, and a write answers what it persisted.
    pub invoke_mode: Option<String>,
    /// ISO-8601 instant the URL was created, "in ISO-8601 format
    /// (YYYY-MM-DDThh:mm:ss.sTZD)" as the model's `CreationTime` member documents.
    pub creation_time: String,
    /// ISO-8601 instant of the last change to this configuration; equal to
    /// [`Self::creation_time`] until one is made.
    pub last_modified_time: String,
}

impl FunctionUrlConfigRecord {
    /// The `InvokeMode` this function URL SERVES IN: the one a caller configured, and
    /// [`DEFAULT_INVOKE_MODE`] for a URL none was ever configured on, because that is the
    /// behaviour the member's documentation gives an unset configuration ("BUFFERED –
    /// This is the default option").
    ///
    /// This is what a READ of the configuration reports and what the data plane invokes
    /// by; the member [`Self::invoke_mode`] holds is what a caller SET, which is what a
    /// write answers.
    #[must_use]
    pub fn served_invoke_mode(&self) -> String {
        self.invoke_mode
            .clone()
            .unwrap_or_else(|| DEFAULT_INVOKE_MODE.to_string())
    }
}

/// `ProvisionedConcurrencyStatusEnum` values an allocation moves through, from the
/// model's enum (`IN_PROGRESS | READY | FAILED`). An allocation this store accepts is
/// `IN_PROGRESS` until the clock makes it `READY`; `FAILED` is the status of an
/// allocation Lambda could not make, and nothing here fails one.
pub const PROVISIONED_CONCURRENCY_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`PROVISIONED_CONCURRENCY_IN_PROGRESS`].
pub const PROVISIONED_CONCURRENCY_READY: &str = "READY";

/// How long after a `PutProvisionedConcurrencyConfig` the allocation it asked for
/// becomes [`PROVISIONED_CONCURRENCY_READY`].
///
/// AWS documents no duration for it — only that the allocation is not immediate ("The
/// status of the allocation process" is `IN_PROGRESS` while Lambda prepares the
/// environments) — so the span is EMULATOR-CHOSEN, the same half second this slice uses
/// for a function's own activation ([`ACTIVATION_DELAY`]). It is never zero, so a caller
/// that reads the configuration straight after setting it sees the `IN_PROGRESS` status
/// and the zero allocation that go with a warm-up still under way.
pub const PROVISIONED_CONCURRENCY_ALLOCATION_DELAY: Duration = Duration::from_millis(500);

/// The message a provisioned-concurrency operation refuses with when the qualifier a
/// configuration would be ADDED to names the unpublished version.
///
/// A configuration "is added to a function's alias or version", and `$LATEST` is neither,
/// so it is never a qualifier one can be added to, read from or removed from. Every one
/// of those operations answers one fixed sentence for it — the sentence names nothing the
/// request carried, so it is the wording of the rule rather than an echo of a call — and
/// the one the write answers says the configuration cannot be applied there, while
/// [`PROVISIONED_CONCURRENCY_QUALIFIER_MESSAGE`] is what the operations addressing an
/// existing configuration answer.
pub const PROVISIONED_CONCURRENCY_CANNOT_APPLY_TO_LATEST_MESSAGE: &str =
    "Provisioned Concurrency Configs cannot be applied to unpublished function versions.";

/// The message an operation addressing an EXISTING configuration refuses with when the
/// qualifier names the unpublished version: the same rule as
/// [`PROVISIONED_CONCURRENCY_CANNOT_APPLY_TO_LATEST_MESSAGE`], stated as the resource form
/// the operation requires.
pub const PROVISIONED_CONCURRENCY_QUALIFIER_MESSAGE: &str =
    "The function resource provided must be an alias or a published version.";

/// The message a provisioned-concurrency operation refuses with when the alias or version
/// it resolved holds no configuration.
///
/// Lambda answers this one fixed sentence for a configuration that does not exist — the
/// request already names which one it asked about — rather than the `Function not found:
/// <arn>` form [`resolve_function`] builds when it is the FUNCTION, alias or version that
/// is missing, so a caller can tell "no such version" from "that version has no
/// provisioned concurrency".
pub const PROVISIONED_CONCURRENCY_CONFIG_NOT_FOUND_MESSAGE: &str =
    "No Provisioned Concurrency Config found for this function";

/// The message a delete of a qualifier that still holds a provisioned concurrency
/// configuration is refused with: `ResourceConflictException`'s own subject ("another
/// operation is in progress", the qualifier's capacity still being held), naming the
/// qualifier the request addressed and the configuration that has to go first.
///
/// A provisioned concurrency configuration is attached to the alias or version it was
/// added to, so that qualifier is IN USE by it: deleting the qualifier out from under an
/// allocation would leave capacity committed to a resource that no longer exists, which
/// is why the configuration is deleted first. `qualifier_arn` is the ARN of the resolved
/// alias or version ([`FunctionRecord::versioned_arn`]), so the sentence names the
/// resource the caller asked to delete and tells it what to delete first.
#[must_use]
pub fn provisioned_concurrency_in_use_message(qualifier_arn: &str) -> String {
    format!(
        "The operation cannot be performed at this time. Provisioned concurrency is \
         configured for {qualifier_arn}. Delete the provisioned concurrency configuration \
         before deleting this resource."
    )
}

/// One provisioned concurrency configuration: the concurrency a
/// `PutProvisionedConcurrencyConfig` call asked Lambda to keep initialised for one alias
/// or version of a function, and how far that allocation has got.
///
/// Held per qualifier on [`FunctionRecord::provisioned_concurrency_configs`], because
/// `PutProvisionedConcurrencyConfig` "Adds a provisioned concurrency configuration to a
/// function's alias or version" and `ProvisionedConcurrencyConfigListItem$FunctionArn`
/// is "The Amazon Resource Name (ARN) of the alias or version" — so an alias and the
/// version it invokes each hold their own configuration, keyed by the qualifier the call
/// addressed.
///
/// The allocation itself is owed by the CLOCK rather than by anybody calling: the record
/// stores the instant it becomes [`PROVISIONED_CONCURRENCY_READY`] and
/// [`ProvisionedConcurrencyRecord::materialize`] brings the status up to whatever the
/// clock has already made due. Nothing sleeps.
#[derive(Debug, Clone)]
pub struct ProvisionedConcurrencyRecord {
    /// "The amount of provisioned concurrency requested", exactly as the call asked for
    /// it.
    pub requested: i32,
    /// One of the model's `ProvisionedConcurrencyStatusEnum` values.
    pub status: String,
    /// When the move to [`PROVISIONED_CONCURRENCY_READY`] becomes due, while the
    /// allocation is still being made.
    pub ready_due: Option<SystemTime>,
    /// "The date and time that a user last updated the configuration, in ISO 8601
    /// format", stamped by the call that stored this record.
    pub last_modified: String,
}

impl ProvisionedConcurrencyRecord {
    /// Bring this record up to whatever the clock has already made due: an allocation
    /// whose warm-up instant has passed is ready, and would have been ready at that same
    /// instant even if nobody had called.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.ready_due.is_some_and(|due| now >= due) {
            self.status = PROVISIONED_CONCURRENCY_READY.to_string();
            self.ready_due = None;
        }
    }

    /// "The amount of provisioned concurrency allocated": all of what was requested once
    /// the allocation is ready, and none of it while Lambda is still making it.
    ///
    /// Read it only after [`Self::materialize`], so an allocation the clock has already
    /// completed is not reported as still empty.
    #[must_use]
    pub fn allocated(&self) -> i32 {
        if self.status == PROVISIONED_CONCURRENCY_READY {
            self.requested
        } else {
            0
        }
    }

    /// What every reader of one configuration reports, built entirely out of what is
    /// stored.
    ///
    /// The model declares ONE `ProvisionedConcurrencyConfig` response shape, generated
    /// here as [`GetProvisionedConcurrencyConfigOutput`]; an operation whose output is
    /// generated as its own struct over that shape carries these members across one for
    /// one rather than rebuilding them from state a second time.
    ///
    /// `StatusReason` is "For failed allocations, the reason that provisioned concurrency
    /// could not be allocated", and no allocation this slice accepts fails, so none is
    /// reported.
    #[must_use]
    pub fn reported(&self) -> GetProvisionedConcurrencyConfigOutput {
        let allocated = self.allocated();
        GetProvisionedConcurrencyConfigOutput {
            requested_provisioned_concurrent_executions: Some(self.requested),
            allocated_provisioned_concurrent_executions: Some(allocated),
            // "The amount of provisioned concurrency available": what is allocated is
            // initialised and therefore available to serve an invocation.
            available_provisioned_concurrent_executions: Some(allocated),
            status: Some(self.status.clone()),
            status_reason: None,
            last_modified: Some(self.last_modified.clone()),
        }
    }

    /// The `ProvisionedConcurrencyConfigListItem` a listing reports for this
    /// configuration: the same members [`Self::reported`] answers, beside
    /// `FunctionArn` — "The Amazon Resource Name (ARN) of the alias or version" the
    /// configuration was added to, which the caller passes `function_arn`.
    #[must_use]
    pub fn listed(&self, function_arn: String) -> ProvisionedConcurrencyConfigListItem {
        let reported = self.reported();
        ProvisionedConcurrencyConfigListItem {
            function_arn: Some(function_arn),
            requested_provisioned_concurrent_executions: reported
                .requested_provisioned_concurrent_executions,
            available_provisioned_concurrent_executions: reported
                .available_provisioned_concurrent_executions,
            allocated_provisioned_concurrent_executions: reported
                .allocated_provisioned_concurrent_executions,
            status: reported.status,
            status_reason: reported.status_reason,
            last_modified: reported.last_modified,
        }
    }
}

/// The qualifier that names the PUBLISHED view of a function's unpublished
/// configuration.
///
/// The model spells it in two places: `PublishedFunctionQualifier`, the shape every
/// function-scaling operation's `Qualifier` targets, admits exactly
/// `^(\$LATEST\.PUBLISHED|[0-9]+)$`, and `NamespacedFunctionName` admits it as an ARN's
/// qualifier beside [`VERSION_LATEST`]. Every function has it, because every function has
/// the unpublished version it is the published view of.
pub const VERSION_LATEST_PUBLISHED: &str = "$LATEST.PUBLISHED";

/// How long after a `PutFunctionScalingConfig` the bounds it asked for become the ones in
/// effect.
///
/// An EMULATOR-CHOSEN span, like [`ACTIVATION_DELAY`]: AWS publishes no duration for it,
/// only the distinction the model's own two members draw between the configuration that
/// was requested and the one "that is currently applied to the function", which
/// "represents the actual scaling settings in effect". It is never zero, so a caller that
/// reads the configuration straight after setting it still sees bounds that are requested
/// and not yet applied.
pub const FUNCTION_SCALING_APPLY_DELAY: Duration = Duration::from_millis(500);

/// The minimum number of execution environments the service itself keeps a Lambda
/// Managed Instances function's qualifier at while nothing else has been asked for.
///
/// Scaling such a function is the SERVICE's job from the moment the function exists —
/// `PutFunctionScalingConfig` only "defines the minimum and maximum number of execution
/// environments that CAN be provisioned" on top of that — so a qualifier nobody has
/// configured still has a requested configuration, and this floor is it. It sits inside
/// the model's `FunctionScalingConfigExecutionEnvironments` range (`0..15000`), and no
/// maximum goes with it, because nothing caps the function until a caller asks for a cap.
pub const DEFAULT_MIN_EXECUTION_ENVIRONMENTS: i32 = 3;

/// The refusal every function-scaling operation answers for a qualifier whose
/// configuration names no capacity provider.
///
/// Both operations are documented for a Lambda Managed Instances function — one that runs
/// on a capacity provider — so a function that runs on none is outside the surface they
/// serve: the call is refused rather than answered for, which is what lets a client tell
/// "this is not a Managed Instances function" from "this function has no scaling bounds".
/// The sentence names nothing of the request: the caller already names the function it
/// asked about.
pub const NO_CAPACITY_PROVIDER_MESSAGE: &str =
    "The function provided by the arn does not contain a capacity provider configuration";

/// The scaling configuration a Managed Instances function's qualifier carries until a
/// `PutFunctionScalingConfig` call stores one of its own: the service-managed floor of
/// [`DEFAULT_MIN_EXECUTION_ENVIRONMENTS`] environments, with no maximum.
#[must_use]
pub fn service_managed_function_scaling_config() -> FunctionScalingConfig {
    FunctionScalingConfig {
        min_execution_environments: Some(DEFAULT_MIN_EXECUTION_ENVIRONMENTS),
        max_execution_environments: None,
    }
}

/// The scaling configuration of one qualifier of a function: the execution-environment
/// bounds a `PutFunctionScalingConfig` call asked for, and the bounds in effect.
///
/// Held per qualifier on [`FunctionRecord::function_scaling_configs`], because the
/// operation takes a `Qualifier` to say which version or published view of the function
/// it sets the configuration for, so each of them holds its own.
///
/// Applying the bounds is owed by the CLOCK rather than by anybody calling: the record
/// stores the instant the requested bounds become the applied ones and
/// [`Self::materialize`] brings it up to whatever the clock has already made due.
#[derive(Debug, Clone, Default)]
pub struct FunctionScalingConfigRecord {
    /// "The scaling configuration to apply to the function", exactly as the most recent
    /// call stated it; a later call REPLACES it, because the operation sets the
    /// configuration rather than adding to one.
    pub requested: FunctionScalingConfig,
    /// The configuration in effect, absent until the first requested one has been
    /// applied — nothing here reports settings as in effect before the clock owes them.
    pub applied: Option<FunctionScalingConfig>,
    /// When [`Self::requested`] becomes the applied configuration, while it is still
    /// being applied; see [`FUNCTION_SCALING_APPLY_DELAY`].
    pub applied_due: Option<SystemTime>,
}

impl FunctionScalingConfigRecord {
    /// Record that `requested` was asked for at `now`: it replaces whatever the previous
    /// call asked for, the bounds in effect are unchanged until the clock makes the new
    /// ones due, and that instant is stored rather than waited for.
    pub fn request(&mut self, requested: FunctionScalingConfig, now: SystemTime) {
        self.requested = requested;
        self.applied_due = Some(now + FUNCTION_SCALING_APPLY_DELAY);
    }

    /// Bring this record up to whatever the clock has already made due: bounds whose
    /// application instant has passed are the ones in effect, and would have been in
    /// effect at that same instant even if nobody had called.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.applied_due.is_some_and(|due| now >= due) {
            self.applied = Some(self.requested.clone());
            self.applied_due = None;
        }
    }

    /// What a reader of one configuration reports, built entirely out of what is stored;
    /// `function_arn` is the ARN of the qualifier the configuration was set on.
    ///
    /// Read it only after [`Self::materialize`], so bounds the clock has already applied
    /// are not reported as still only requested.
    #[must_use]
    pub fn reported(&self, function_arn: String) -> GetFunctionScalingConfigOutput {
        GetFunctionScalingConfigOutput {
            function_arn: Some(function_arn),
            applied_function_scaling_config: self.applied.clone(),
            // "The scaling configuration that was requested for the function" is the one
            // still waiting to become the one in effect: a request is reported while it is
            // outstanding, and stops being reported once it IS
            // "the scaling configuration that is currently applied to the function", which
            // from then on is the only configuration there is to report. Its absence is
            // therefore what tells a caller its request landed.
            requested_function_scaling_config: self
                .applied_due
                .is_some()
                .then(|| self.requested.clone()),
        }
    }
}

/// The runtime management configuration of one function version: "the runtime update
/// mode" and, in [`UPDATE_RUNTIME_ON_MANUAL`] mode only, the runtime version the
/// version is pinned to.
///
/// Held per version label on [`FunctionRecord::runtime_management_configs`], because
/// `PutRuntimeManagementConfig` "sets the runtime management configuration for a
/// function's version" and takes a `Qualifier` to say which one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeManagementConfigRecord {
    /// One of the model's `UpdateRuntimeOn` values, as supplied.
    pub update_runtime_on: String,
    /// The pinned runtime version ARN, as supplied. Present only in
    /// [`UPDATE_RUNTIME_ON_MANUAL`] mode — "The ARN of the runtime the function is
    /// configured to use. If the runtime update mode is manual, the ARN is returned,
    /// otherwise null is returned" — and never invented here, because no mode but
    /// `Manual` names one.
    pub runtime_version_arn: Option<String>,
}

impl Default for RuntimeManagementConfigRecord {
    /// The configuration a version carries until a `PutRuntimeManagementConfig` call
    /// stores another: [`DEFAULT_UPDATE_RUNTIME_ON`], with no pinned runtime version.
    fn default() -> Self {
        Self {
            update_runtime_on: DEFAULT_UPDATE_RUNTIME_ON.to_string(),
            runtime_version_arn: None,
        }
    }
}

/// The most durable executions this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`]: the executions
/// are held in memory, every start adds one keyed by the name the caller chose and
/// nothing ever removes one, so past this point a start is REFUSED rather than the store
/// growing without bound. It refuses rather than making room the way the asynchronous
/// queue does ([`MAX_QUEUED_ASYNC_INVOCATIONS`]), because `ListDurableExecutionsByFunction`
/// reports every execution that was started: dropping one would un-report work this
/// emulator accepted. AWS bounds a function's executions by its concurrency quotas
/// instead.
pub const MAX_DURABLE_EXECUTIONS: usize = 1_000;

/// The model's `ExecutionStatus` value a durable execution carries while it is still
/// running (`smithy_query(query="shape", shape="ExecutionStatus")`: `RUNNING`,
/// `SUCCEEDED`, `FAILED`, `TIMED_OUT`, `STOPPED`).
pub const EXECUTION_STATUS_RUNNING: &str = "RUNNING";

/// The `ExecutionStatus` of an execution that finished its work; see
/// [`EXECUTION_STATUS_RUNNING`].
pub const EXECUTION_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The `ExecutionStatus` of an execution that ended in an error; see
/// [`EXECUTION_STATUS_RUNNING`].
pub const EXECUTION_STATUS_FAILED: &str = "FAILED";

/// The `ExecutionStatus` of an execution that ran past its `ExecutionTimeout`; see
/// [`EXECUTION_STATUS_RUNNING`] and [`DurableExecutionRecord::timeout_due`].
pub const EXECUTION_STATUS_TIMED_OUT: &str = "TIMED_OUT";

/// The `ExecutionStatus` of an execution a caller stopped; see
/// [`EXECUTION_STATUS_RUNNING`].
pub const EXECUTION_STATUS_STOPPED: &str = "STOPPED";

/// The ARN infix a durable execution's own ARN carries, from the model's
/// `DurableExecutionArn` pattern
/// (`…:function:<name>:<qualifier>/durable-execution/<execution-name>/<execution-id>`).
pub const DURABLE_EXECUTION_ARN_INFIX: &str = "durable-execution";

/// The model's `EventType` for the event a durable execution's history opens with
/// (`smithy_query(query="shape", shape="EventType")`), carrying the input the execution
/// started from and the timeout it runs under.
pub const EVENT_TYPE_EXECUTION_STARTED: &str = "ExecutionStarted";

/// The model's `EventType` for the event an execution that ran past its
/// `ExecutionTimeout` closes with; see [`EVENT_TYPE_EXECUTION_STARTED`].
pub const EVENT_TYPE_EXECUTION_TIMED_OUT: &str = "ExecutionTimedOut";

/// The model's `EventType` for the event an execution whose function RETURNED closes with,
/// carrying the result it returned; see [`EVENT_TYPE_EXECUTION_STARTED`].
pub const EVENT_TYPE_EXECUTION_SUCCEEDED: &str = "ExecutionSucceeded";

/// The model's `EventType` for the event an execution whose function FAILED closes with,
/// carrying the error it failed on; see [`EVENT_TYPE_EXECUTION_STARTED`].
pub const EVENT_TYPE_EXECUTION_FAILED: &str = "ExecutionFailed";

/// The model's `EventType` for the event an execution a caller STOPPED closes with; see
/// [`EVENT_TYPE_EXECUTION_STARTED`] and [`DurableExecutionRecord::stop`].
pub const EVENT_TYPE_EXECUTION_STOPPED: &str = "ExecutionStopped";

/// One durable execution of a function.
///
/// Held per function on [`FunctionRecord::durable_executions`] and keyed by its name,
/// because a durable execution's name identifies it within the function it belongs to:
/// `Invoke`'s own `DurableExecutionAlreadyStartedException` says "Each durable execution
/// name must be unique within the function".
///
/// The execution is a resource with a lifetime of its own. It starts
/// [`EXECUTION_STATUS_RUNNING`], and the move to [`EXECUTION_STATUS_TIMED_OUT`] is owed
/// by the CLOCK rather than by anybody calling: `DurableConfig$ExecutionTimeout` is "the
/// maximum time (in seconds) that a durable execution can run before timing out. This
/// timeout applies to the entire durable execution", so the record stores the instant
/// that timeout falls due and [`DurableExecutionRecord::materialize`] applies whatever
/// the clock has already made due. Nothing here ever reports a result the execution did
/// not produce.
#[derive(Debug, Clone)]
pub struct DurableExecutionRecord {
    /// The execution's name, as the call that started it named it.
    pub name: String,
    /// The id this emulator minted for the execution, which its ARN carries beside the
    /// name; see [`mint_durable_execution_id`].
    pub execution_id: String,
    /// The version label the execution runs under (`$LATEST` or a published version),
    /// which its ARN carries and which `ListDurableExecutionsByFunction`'s `Qualifier`
    /// filters on.
    pub version: String,
    /// The model's `ExecutionStatus` the execution currently carries.
    pub status: String,
    /// When the execution started, which the model's `StartTimestamp` reports.
    pub started: SystemTime,
    /// The payload the call that started the execution carried, which the model's
    /// `InputPayload` reports and which a later start of the SAME name is compared
    /// against: `Invoke$DurableExecutionName` documents that a repeat "with the same
    /// payload" answers the execution already started and a repeat whose "payload differs"
    /// is refused, so the payload is part of the execution's identity.
    ///
    /// What it can hold is bounded on both sides: one payload by the invocation payload
    /// ceiling `Invoke` checks BEFORE a start is attempted, and the number of them by
    /// [`MAX_DURABLE_EXECUTIONS`] per function.
    pub input_payload: Vec<u8>,
    /// When the execution ended, once it reached a terminal status; the model's
    /// `EndTimestamp`, which is absent while it is still running.
    pub ended: Option<SystemTime>,
    /// Why the execution ended, once it ended for a reason a caller gave: the model's
    /// `GetDurableExecution$Error`, "An object that contains error information".
    ///
    /// It is only ever the error object the call that ENDED the execution carried
    /// (`StopDurableExecution$Error`, "Optional error details explaining why the
    /// execution is being stopped"), so no read ever reports an error the execution did
    /// not end with.
    pub error: Option<ErrorObject>,
    /// The customer managed key the function's `DurableConfig` named for the execution's
    /// payload data, when it named one.
    pub kms_key_arn: Option<String>,
    /// The result the execution's function RETURNED, once it ended successfully: "The JSON
    /// result returned by the durable execution if it completed successfully. This field is
    /// only present when the execution status is `SUCCEEDED`."
    ///
    /// It is only ever what the RUN of the function produced
    /// ([`DurableExecutionRecord::settle`]), so no read reports a result the execution did
    /// not produce.
    pub result: Option<String>,
    /// When the execution's `ExecutionTimeout` falls due, while it is running and the
    /// function's `DurableConfig` set one.
    pub timeout_due: Option<SystemTime>,
    /// Everything that has happened to this execution, oldest first, which
    /// `GetDurableExecutionHistory` reports: "Retrieves the execution history for a
    /// durable execution, showing all the steps, callbacks, and events that occurred
    /// during the execution."
    ///
    /// The log is APPEND-ONLY and it is the only record of what happened: an event is
    /// appended by the call that made it happen (the start, and the stop), or by the clock
    /// transition that fell due ([`DurableExecutionRecord::materialize`]), so nothing
    /// reading the history ever invents an event. `EventId` is "the unique identifier for
    /// this event. Event IDs increment sequentially", which is the position in this
    /// vector, counted from 1 ([`DurableExecutionRecord::next_event`]).
    ///
    /// It is bounded by what can append to it: one event for the start, and one for the
    /// TERMINAL transition — the timeout the clock owes, or the stop a caller makes —
    /// which can happen only once, because neither moves an execution that has already
    /// ended.
    pub history: Vec<Event>,
}

impl DurableExecutionRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transition it applies is the one the clock owed at `now`: a running
    /// execution whose `ExecutionTimeout` has fallen due has run past "the maximum time
    /// (in seconds) that a durable execution can run before timing out", so it is
    /// [`EXECUTION_STATUS_TIMED_OUT`] and it ended when that timeout fell due. It would
    /// have been owed at the same instant even if nobody had called.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.status != EXECUTION_STATUS_RUNNING {
            return;
        }
        if let Some(due) = self.timeout_due.filter(|due| now >= *due) {
            self.status = EXECUTION_STATUS_TIMED_OUT.to_string();
            self.ended = Some(due);
            self.timeout_due = None;
            // The history records what happened, so the transition the clock owed is
            // appended as the event it is, timed at the instant it fell due rather than
            // at the call that noticed it. `ExecutionTimedOutDetails$Error` is optional
            // and no in-process runtime here produced an error payload for the
            // execution, so none is reported.
            let mut event = self.next_event(EVENT_TYPE_EXECUTION_TIMED_OUT, due);
            event.execution_timed_out_details = Some(ExecutionTimedOutDetails { error: None });
            self.history.push(event);
        }
    }

    /// Settle this execution on what the RUN of its function came to, at `now`.
    ///
    /// A durable execution is the handle on ONE invocation of a durable function, so the
    /// status it ends in is the one that invocation's run produced: a handler that returned
    /// makes the execution [`EXECUTION_STATUS_SUCCEEDED`] carrying the result it returned,
    /// and a run that failed — a handler that raised, a run its timeout ended, a run this
    /// emulator could not make at all — makes it [`EXECUTION_STATUS_FAILED`] carrying the
    /// error that failure is. Nothing here reports a result or an error the run did not
    /// produce.
    ///
    /// An execution already in a terminal status is NOT moved and nothing is appended: a
    /// caller's `StopDurableExecution` and the timeout the clock owed both end it for good,
    /// and the run of a stopped execution cannot re-open it.
    pub fn settle(&mut self, outcome: &DurableOutcome, now: SystemTime) {
        if self.status != EXECUTION_STATUS_RUNNING {
            return;
        }
        self.ended = Some(now);
        self.timeout_due = None;
        // The history records what happened, so the terminal transition is appended as the
        // event it is, carrying what the run produced. `Truncated` "indicates if the …
        // payload was truncated due to size limits": the whole result the run produced is
        // kept, so nothing here is truncated.
        match outcome {
            DurableOutcome::Succeeded(result) => {
                self.status = EXECUTION_STATUS_SUCCEEDED.to_string();
                self.result = result.clone();
                let mut event = self.next_event(EVENT_TYPE_EXECUTION_SUCCEEDED, now);
                event.execution_succeeded_details = Some(ExecutionSucceededDetails {
                    result: EventResult {
                        payload: result.clone(),
                        truncated: Some(false),
                    },
                });
                self.history.push(event);
            }
            DurableOutcome::Failed(error) => {
                self.status = EXECUTION_STATUS_FAILED.to_string();
                self.error = Some(error.clone());
                let mut event = self.next_event(EVENT_TYPE_EXECUTION_FAILED, now);
                event.execution_failed_details = Some(ExecutionFailedDetails {
                    error: EventError {
                        payload: Some(error.clone()),
                        truncated: Some(false),
                    },
                });
                self.history.push(event);
            }
        }
    }

    /// Stop this execution at `now`, for the reason `error` gives, and report the instant
    /// it stopped.
    ///
    /// "Stops a running durable execution. The execution transitions to STOPPED status and
    /// cannot be resumed. Any in-progress operations are terminated" — so a RUNNING
    /// execution becomes [`EXECUTION_STATUS_STOPPED`], ends at `now`, keeps the error the
    /// caller gave as the reason it ended, and owes no timeout any more.
    ///
    /// An execution already in a terminal status is NOT moved and nothing is appended: it
    /// cannot be resumed, so it cannot be stopped a second time, and the model declares no
    /// state error for the attempt. The instant reported is then the one the execution
    /// really ended at, so a repeat of the same stop answers the same `StopTimestamp` as
    /// the stop that performed it.
    pub fn stop(&mut self, error: Option<ErrorObject>, now: SystemTime) -> SystemTime {
        if self.status != EXECUTION_STATUS_RUNNING {
            return self.ended.unwrap_or(now);
        }
        self.status = EXECUTION_STATUS_STOPPED.to_string();
        self.ended = Some(now);
        self.timeout_due = None;
        self.error = error.clone();
        // The history records what happened, and `ExecutionStoppedDetails$Error` is
        // REQUIRED: the event carries the error object the stop named, and carries none
        // inside that envelope for a stop that named no reason. The whole object the call
        // sent is kept, so nothing about it is truncated.
        let mut event = self.next_event(EVENT_TYPE_EXECUTION_STOPPED, now);
        event.execution_stopped_details = Some(ExecutionStoppedDetails {
            error: EventError {
                payload: error,
                truncated: Some(false),
            },
        });
        self.history.push(event);
        now
    }

    /// The next history event of this execution: `event_type` at the instant `at` it
    /// happened, numbered by the events already recorded.
    ///
    /// `EventId` is "the unique identifier for this event. Event IDs increment
    /// sequentially", so it is the event's one-based position in
    /// [`DurableExecutionRecord::history`]; the caller fills in the one details member
    /// its type carries and appends the event.
    #[must_use]
    pub fn next_event(&self, event_type: &str, at: SystemTime) -> Event {
        Event {
            event_type: Some(event_type.to_string()),
            event_id: i32::try_from(self.history.len() + 1).unwrap_or(i32::MAX),
            event_timestamp: Some(::aws_smithy_types::DateTime::from(at)),
            ..Event::default()
        }
    }

    /// Whether this execution started strictly after `at`, which is what
    /// `ListDurableExecutionsByFunction`'s `StartedAfter` selects ("Filter executions
    /// that started after this timestamp").
    #[must_use]
    pub fn started_after(&self, at: ::aws_smithy_types::DateTime) -> bool {
        ::aws_smithy_types::DateTime::from(self.started).as_secs_f64() > at.as_secs_f64()
    }

    /// Whether this execution started strictly before `at`, which is what
    /// `ListDurableExecutionsByFunction`'s `StartedBefore` selects ("Filter executions
    /// that started before this timestamp").
    #[must_use]
    pub fn started_before(&self, at: ::aws_smithy_types::DateTime) -> bool {
        ::aws_smithy_types::DateTime::from(self.started).as_secs_f64() < at.as_secs_f64()
    }
}

/// The `EventInput` a history event reports for the payload an execution was started
/// from.
///
/// The model types `InputPayload` as a STRING, so a payload that is not text is reported
/// as no payload at all rather than as mangled text — the same reading
/// `GetDurableExecution` gives the stored payload. `Truncated` "indicates if the error
/// payload was truncated due to size limits": the whole payload the start carried is
/// stored, so nothing here is ever truncated.
#[must_use]
pub fn event_payload(payload: &[u8]) -> EventInput {
    EventInput {
        payload: String::from_utf8(payload.to_vec())
            .ok()
            .filter(|payload| !payload.is_empty()),
        truncated: Some(false),
    }
}

/// A fresh durable execution id.
///
/// The model's `DurableExecutionArn` pattern ends in
/// `/durable-execution/<execution-name>/([a-zA-Z0-9_-]+)`, so the id is the lower-case
/// hexadecimal of a fresh UUID: unique per execution and inside that pattern.
#[must_use]
pub fn mint_durable_execution_id() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// A fresh durable execution NAME, for a start that named none.
///
/// `Invoke$DurableExecutionName` is optional and documents the default: "If not
/// specified, Lambda generates a unique identifier automatically." The generated name is
/// a fresh UUID's hexadecimal, which is unique and satisfies that member's own
/// `@length(1, 64)` and `^[a-zA-Z0-9-_]+$`.
#[must_use]
pub fn mint_durable_execution_name() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}

/// What the RUN of a durable execution's function came to, which is what the execution is
/// settled on ([`DurableExecutionRecord::settle`]).
#[derive(Debug, Clone)]
pub enum DurableOutcome {
    /// The handler returned; the JSON result it produced, when it produced text at all.
    Succeeded(Option<String>),
    /// The run did not produce a result: the error it ended on.
    Failed(ErrorObject),
}

/// What became of a [`FunctionRecord::start_durable_execution`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableExecutionStart {
    /// A new execution was stored; the ARN it is addressed by.
    Started(String),
    /// The function already held an execution of that name carrying the same payload, so
    /// nothing was stored and that execution answers the start: "Lambda returns the
    /// existing execution instead of creating a duplicate."
    AlreadyStarted(String),
    /// The function already held an execution of that name carrying a DIFFERENT payload,
    /// which is the state the modelled `DurableExecutionAlreadyStartedException`
    /// describes. Nothing was stored.
    NameAlreadyStarted,
    /// The function already holds [`MAX_DURABLE_EXECUTIONS`], so the start was refused
    /// and nothing was stored.
    LimitExceeded,
}

/// The most aliases this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling, in the sense [`MAX_FUNCTIONS`] is: the store is in memory
/// and every `CreateAlias` adds to it. AWS bounds the aliases of a function too and
/// answers the modelled `AliasLimitExceededException` for the create that would pass the
/// bound ("Lambda couldn't create the alias because your Amazon Web Services account has
/// exceeded the maximum number of aliases allowed per Lambda function"), but neither the
/// model nor the published quotas carry the number, so this slice picks one and answers
/// that same modelled error past it.
///
/// The number is picked above the counts a caller is observed to reach: AWS accepts more
/// than one hundred aliases on a single function, so a ceiling at a hundred would refuse
/// creates AWS serves. It therefore sits where [`MAX_FUNCTIONS`] does, high enough that
/// the bound is the store's rather than the API's.
pub const MAX_FUNCTION_ALIASES: usize = 1_000;

/// The most versions one alias may route to BESIDE the version it invokes.
///
/// `CreateAlias`'s own documentation describes traffic shifting as a split between two
/// versions — "You can also map an alias to split invocation requests between two
/// versions. Use the RoutingConfig parameter to specify a second version and the
/// percentage of invocation requests that it receives" — so `AdditionalVersionWeights`
/// names at most one.
pub const MAX_ALIAS_ADDITIONAL_VERSIONS: usize = 1;

/// The model's `Alias` pattern, which every operation naming an alias puts on it.
pub const ALIAS_NAME_PATTERN: &str = r"^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$";

/// Whether `name` is a name an alias may carry.
///
/// An alias shares the qualifier namespace with a function's published version numbers,
/// and [`ALIAS_NAME_PATTERN`] is what keeps the two apart: its negative lookahead
/// refuses an all-digit name, so no alias can ever be confused with a version.
#[must_use]
pub fn is_alias_name(name: &str) -> bool {
    !name.is_empty()
        && !name.bytes().all(|byte| byte.is_ascii_digit())
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
        })
}

/// The refusal for a name no alias may carry: a validation message built from the
/// request, naming the member and the constraint it failed.
///
/// Both operations that take an `Alias`-targeted member spell the member `Name`, which
/// a message states as `name`.
#[must_use]
pub fn alias_name_message(name: &str) -> String {
    pattern_violation_message("name", name, ALIAS_NAME_PATTERN)
}

/// The message the READ of an alias refuses with when the function holds no alias of that
/// name: `ResourceNotFoundException`'s own sentence, naming the alias by the
/// fully-qualified ARN the request asked about.
///
/// `alias_arn` is the function's unqualified ARN qualified by the alias name
/// ([`FunctionRecord::versioned_arn`]), so the whole sentence is built from the request:
/// the caller recovers the ARN it asked for by stripping the fixed prefix, which is how
/// every Lambda resource-shaped refusal states the resource it could not find (compare
/// the `Function not found: <arn>` [`resolve_function`] builds).
///
/// Lambda words this absence per operation rather than once for the resource kind, so the
/// WRITE of an alias states the same ARN after its own prefix
/// ([`alias_update_not_found_message`]), exactly as a layer version addressed through its
/// policy states a sentence the read of that version does not.
#[must_use]
pub fn alias_not_found_message(alias_arn: &str) -> String {
    format!("Cannot find alias arn: {alias_arn}")
}

/// The message an operation that UPDATES an alias refuses with when the function holds no
/// alias of that name, which is a different sentence from the one the read answers
/// ([`alias_not_found_message`]).
///
/// `alias_arn` is the same fully-qualified alias ARN
/// ([`FunctionRecord::versioned_arn`]), so both sentences name the alias the request asked
/// about and only the fixed lead-in tells them apart.
#[must_use]
pub fn alias_update_not_found_message(alias_arn: &str) -> String {
    format!("Alias not found: {alias_arn}")
}

/// The message a delete of a published version that one or more aliases still reference
/// is refused with: `ResourceConflictException`'s own subject — the version is still in
/// use — naming every alias that has to be repointed or deleted first.
///
/// "You can't delete a version that an alias references"
/// (`DeleteFunction$Qualifier`), so the sentence states which aliases do: the names are
/// listed inside one bracketed group, comma-separated, in the order the function keeps
/// its aliases ([`FunctionRecord::aliases_referencing`]), so the caller reads the whole
/// set it has to deal with from one refusal rather than one name per retry.
#[must_use]
pub fn version_referenced_by_aliases_message(aliases: &[String]) -> String {
    format!(
        "Unable to delete version because the following aliases reference it: [{}]",
        aliases.join(", ")
    )
}

/// The refusal a `RoutingConfig` naming more than
/// [`MAX_ALIAS_ADDITIONAL_VERSIONS`] additional versions gets.
///
/// Fixed text naming the member and the ceiling: an alias splits its invocations between
/// the version it invokes and ONE other, so the count the request sent says nothing the
/// sentence needs.
pub const TOO_MANY_ADDITIONAL_VERSIONS_MESSAGE: &str =
    "Number of items in AdditionalVersionWeights cannot be greater than 1";

/// The refusal a `RoutingConfig` on an alias that invokes the UNPUBLISHED version gets.
///
/// Traffic is shifted between two SNAPSHOTS of the function, and the unpublished version
/// is not one: its code and configuration keep changing under the weights, which is why
/// an alias that routes invocations to a second version has to invoke a published one.
pub const WEIGHTED_ALIAS_ON_UNPUBLISHED_MESSAGE: &str =
    "Weighted alias cannot be mapped to $LATEST";

/// The refusal a `RoutingConfig` whose versions do not share one execution role gets.
pub const WEIGHTED_ALIAS_ROLE_MISMATCH_MESSAGE: &str =
    "Weighted alias cannot be mapped to versions with different execution roles";

/// Whether the two versions an alias would split its invocations between run as DIFFERENT
/// execution roles, and so the sentence refusing that alias — `None` when they run as the
/// same role, and for a version this store holds no configuration for, which the caller
/// has already refused as a version the function does not have.
///
/// An alias is ONE identity to the clients that invoke it, and a function RUNS AS the
/// execution role its configuration names (`FunctionConfiguration$Role`), so two versions
/// under one alias that name different roles would give that one identifier two sets of
/// permissions, chosen by how an invocation happened to be routed.
#[must_use]
pub fn weighted_versions_role_refusal(
    record: &FunctionRecord,
    primary: &str,
    additional: &str,
) -> Option<String> {
    let primary_role = &record.config_of(primary)?.role;
    let additional_role = &record.config_of(additional)?.role;
    (primary_role != additional_role).then(|| WEIGHTED_ALIAS_ROLE_MISMATCH_MESSAGE.to_string())
}

/// One alias of a function: "a function identifier that you can update to invoke a
/// different version", as `CreateAlias` documents it.
///
/// Held per name on [`FunctionRecord::aliases`], because an alias belongs to the
/// function rather than to one of its versions, and a function has at most one alias of
/// a given name.
#[derive(Debug, Clone)]
pub struct AliasRecord {
    /// The name the alias is addressed by, which is also the qualifier of its ARN.
    pub name: String,
    /// "The function version that the alias invokes": [`VERSION_LATEST`] or a published
    /// version number.
    pub function_version: String,
    /// The description, exactly as supplied; absent when the caller gave none.
    pub description: Option<String>,
    /// The second version this alias routes part of its invocations to, and the share it
    /// receives, keyed by version number so the configuration reports in a stable order.
    /// Empty when the alias does no traffic shifting.
    pub additional_version_weights: BTreeMap<String, f64>,
    /// "A unique identifier that changes when you update the alias", minted on every
    /// write to it.
    pub revision_id: String,
}

impl AliasRecord {
    /// The `AliasRoutingConfiguration` the wire reports for this alias, present only
    /// once the alias routes to a second version: the model gives the member no
    /// default, so an alias that shifts no traffic carries no routing configuration at
    /// all.
    #[must_use]
    pub fn routing_config(&self) -> Option<AliasRoutingConfiguration> {
        (!self.additional_version_weights.is_empty()).then(|| AliasRoutingConfiguration {
            additional_version_weights: Some(
                self.additional_version_weights
                    .iter()
                    .map(|(version, weight)| (version.clone(), *weight))
                    .collect(),
            ),
        })
    }
}

/// WHICH VERSION one invocation of an alias runs on: the traffic split an alias's routing
/// configuration performs, applied per invocation. Implements
/// [`AliasRecord::routed_version`] and [`FunctionRecord::routed_version`]; see the module's
/// own header.
pub mod alias_routing;

/// The CONCURRENCY one invocation holds while it runs: the slot an admitted invocation takes,
/// the admission that hands it out and the throttle for one that cannot be admitted; see the
/// module's own header.
pub mod execution_slot;
pub use execution_slot::{
    admit_invocation, Admitted, ExecutionSlot, Throttled, ACCOUNT_CONCURRENCY_REASON,
};

/// The FUNCTION RECORD: one Lambda function as this emulator holds it, and everything a
/// read of that function reports out of what is stored.
///
/// Held in its own module so this one stays readable; the record is re-exported here, so a
/// handler names it as `crate::state::FunctionRecord` exactly as before. The store that
/// HOLDS the records ([`ComAmazonawsLambdaAwsgirapiserviceState::functions`]) stays here,
/// as do the records one function carries and the ARN and message helpers it reads.
pub mod function_record;
pub use function_record::FunctionRecord;

/// The `S3ObjectStorageMode` a layer archive is stored with when the caller names
/// none, from the member's own documentation: "`COPY` (default) – Uploads a copy of
/// your layer archive to Lambda."
pub const S3_OBJECT_STORAGE_MODE_COPY: &str = "COPY";

/// The `S3ObjectStorageMode` that leaves the archive where the caller put it:
/// "`REFERENCE` – Lambda references the layer archive from the specified Amazon S3
/// bucket."
pub const S3_OBJECT_STORAGE_MODE_REFERENCE: &str = "REFERENCE";

/// The most layers this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`]: the store is in
/// memory and every `PublishLayerVersion` adds to it, so past this point the slice
/// answers the modelled `CodeStorageExceededException` rather than growing without
/// bound. AWS bounds layers by the account's code-storage quota instead, and answers the
/// same modelled error.
pub const MAX_LAYERS: usize = 1_000;

/// The most versions this emulator keeps for one layer, bounding the store exactly as
/// [`MAX_FUNCTION_VERSIONS`] bounds a function's versions.
pub const MAX_LAYER_VERSIONS: usize = 500;

/// Where the archive of one layer version came from.
///
/// `zip_bytes` is the only variant whose bytes this emulator holds, so it is the only
/// one a size and a digest can be computed from; an S3 object is a reference this
/// emulator does not resolve, and nothing is reported for it that was not computed.
#[derive(Debug, Clone, Default)]
pub struct LayerArchiveRecord {
    /// The layer archive bytes, when the caller uploaded them inline.
    pub zip_bytes: Option<Vec<u8>>,
    /// The S3 bucket holding the layer archive, when the caller named one.
    pub s3_bucket: Option<String>,
    /// The S3 key of the layer archive.
    pub s3_key: Option<String>,
    /// The S3 object version of the layer archive.
    pub s3_object_version: Option<String>,
    /// How the archive is stored: [`S3_OBJECT_STORAGE_MODE_COPY`] or
    /// [`S3_OBJECT_STORAGE_MODE_REFERENCE`].
    pub s3_object_storage_mode: String,
}

/// One version of a layer: the archive it was published from, the metadata the request
/// carried and when it was published.
///
/// "Each time you call PublishLayerVersion with the same layer name, a new version is
/// created" (the model's own `PublishLayerVersion` documentation), so a version is
/// immutable once stored and keeps its own copy of everything a read reports.
#[derive(Debug, Clone)]
pub struct LayerVersionRecord {
    /// The version number, counting from 1 upwards.
    pub version: i64,
    /// The description of the version, when the caller gave one.
    pub description: Option<String>,
    /// The layer's software license, when the caller gave one.
    pub license_info: Option<String>,
    /// The runtimes the version declares itself compatible with, in the order given.
    pub compatible_runtimes: Vec<String>,
    /// The architectures the version declares itself compatible with, in the order
    /// given.
    pub compatible_architectures: Vec<String>,
    /// Where the archive came from.
    pub archive: LayerArchiveRecord,
    /// The size of the stored archive, in bytes; 0 when this emulator holds no bytes
    /// for it (an S3 object it does not resolve).
    pub code_size: i64,
    /// The base64 SHA-256 of the stored archive bytes; absent when this emulator holds
    /// no bytes to hash.
    pub code_sha256: Option<String>,
    /// ISO-8601 instant the version was published.
    pub created: String,
    /// The resource-based policy of this version, once a statement has been added to
    /// it.
    ///
    /// `AddLayerVersionPermission` "Adds permissions to the resource-based policy of a
    /// version of an Lambda layer", so the policy belongs to the single version the call
    /// addressed rather than to the layer as a whole, and `GetLayerVersionPolicy` reports
    /// exactly the statements that were added.
    ///
    /// The policy is a RECORD the first grant creates, and it outlives its last
    /// statement: removing every statement leaves the version holding an EMPTY policy
    /// rather than none at all. The two states are distinguishable — a version nobody has
    /// granted anything on has no policy record, and a READ of each answers its own
    /// refusal (see [`LayerVersionRecord::policy_read_absence_message`]) — so a caller can
    /// tell a version that never had a policy from one whose statements have all been
    /// taken out. Neither state is reported as a policy: both are the read's 404.
    pub policy: Option<PolicyRecord>,
}

impl LayerVersionRecord {
    /// Why a READ of this version's resource-based policy has nothing to report, as the
    /// message of its refusal — and `None` when the version holds one.
    ///
    /// The version itself EXISTS in both cases (it was resolved out of the store), so
    /// neither message is the one for an absent layer version, which is what lets a
    /// caller tell "no policy yet" (add one) from "wrong ARN" (fix the ARN). The two no-
    /// policy states are distinguished by whether the version has a policy RECORD at all:
    /// a version nobody has granted anything on has none, and one whose statements have
    /// all been removed has an empty one, which the read names the version for.
    #[must_use]
    pub fn policy_read_absence_message(&self, layer_version_arn: &str) -> Option<String> {
        match &self.policy {
            None => Some(NO_POLICY_MESSAGE.to_string()),
            Some(policy) if policy.statements.is_empty() => Some(
                layer_version_without_policy_message(layer_version_arn),
            ),
            Some(_) => None,
        }
    }

    /// The size of this version's archive once EXPANDED, in bytes: the quantity Lambda
    /// measures a function's layers against its unzipped-size limit, measured off the
    /// stored archive by [`unzipped_bytes`] rather than held as a second copy of a number
    /// the archive itself carries. 0 for a version whose bytes this emulator does not hold,
    /// exactly as [`LayerVersionRecord::code_size`] is.
    #[must_use]
    pub fn unzipped_code_size(&self) -> i64 {
        unzipped_bytes(self.archive.zip_bytes.as_deref())
    }

    /// The `LayerVersionContentOutput` a read reports for this version's archive,
    /// `location` being the link into the Lambda-held copy of it.
    #[must_use]
    pub fn content(&self, location: String) -> LayerVersionContentOutput {
        LayerVersionContentOutput {
            location: Some(location),
            code_sha256: self.code_sha256.clone(),
            code_size: self.code_size,
            // `UncompressedCodeSize` is the size of the archive's contents as Lambda has
            // EXPANDED them, and a layer archive is stored exactly as it was uploaded —
            // neither storage mode unpacks it — so no expanded copy of it exists to
            // measure and the member carries its own default. RECORDED: AWS carries the
            // member on every layer version it reports, with the value 0.
            uncompressed_code_size: 0,
            // "The resolved Amazon S3 object that contains the layer archive" — in
            // `REFERENCE` mode that object is the caller's, because "Lambda references
            // the layer archive from the specified Amazon S3 bucket". In the default
            // `COPY` mode Lambda holds a copy of its own and the caller's object is no
            // longer where the archive is served from, so none is reported.
            resolved_s3_object: self
                .archive
                .s3_bucket
                .clone()
                .filter(|_| self.archive.s3_object_storage_mode == S3_OBJECT_STORAGE_MODE_REFERENCE)
                .map(|bucket| ResolvedS3Object {
                    s3_bucket: Some(bucket),
                    s3_key: self.archive.s3_key.clone(),
                    s3_object_version: self.archive.s3_object_version.clone(),
                }),
            ..LayerVersionContentOutput::default()
        }
    }

    /// Whether this version declares itself compatible with `runtime` and with
    /// `architecture`, which is the filter `ListLayers` and `ListLayerVersions` share:
    /// "Specify a runtime identifier to list only layers that indicate that they're
    /// compatible with that runtime. Specify a compatible architecture to include only
    /// layers that are compatible with that instruction set architecture."
    ///
    /// A filter the caller did not name matches every version. A version that declares
    /// no compatible runtime (or architecture) at all indicates compatibility with
    /// none, so a request filtered to one does not match it.
    #[must_use]
    pub fn is_compatible_with(&self, runtime: Option<&str>, architecture: Option<&str>) -> bool {
        runtime.is_none_or(|runtime| {
            self.compatible_runtimes
                .iter()
                .any(|declared| declared == runtime)
        }) && architecture.is_none_or(|architecture| {
            self.compatible_architectures
                .iter()
                .any(|declared| declared == architecture)
        })
    }
}

/// One layer and every version published under its name.
#[derive(Debug, Clone)]
pub struct LayerRecord {
    /// The layer's own ARN, which carries no version.
    pub arn: String,
    /// The region the layer was published in, which its ARNs carry.
    pub region: String,
    /// The bare layer name (never an ARN).
    pub name: String,
    /// The highest version number ever published under this name; a number is never
    /// reused, so the next version follows this one rather than the count held.
    pub latest_version: i64,
    /// Every version this emulator holds, keyed by version number so a read of a
    /// layer's versions is ordered.
    pub versions: BTreeMap<i64, LayerVersionRecord>,
}

impl LayerRecord {
    /// The ARN of `version` of this layer: the layer ARN with the version appended, as
    /// the model's `LayerVersionArn` pattern requires.
    #[must_use]
    pub fn version_arn(&self, version: i64) -> String {
        format!("{}:{version}", self.arn)
    }

    /// The `GetLayerVersionResponse` the wire reports for one of this layer's versions,
    /// built entirely out of what is stored.
    ///
    /// `ctx` is what the archive's `Content$Location` is announced against: the link is one
    /// the caller follows, so it is presigned for the base URL this emulator is reached on
    /// (see [`layer_archive_location`]).
    #[must_use]
    pub fn version_detail(
        &self,
        ctx: &smithy_server_runtime::OperationContext<'_>,
        version: &LayerVersionRecord,
    ) -> GetLayerVersionOutput {
        GetLayerVersionOutput {
            content: Some(version.content(layer_archive_location(
                ctx,
                &self.region,
                &self.name,
                version.version,
            ))),
            layer_arn: Some(self.arn.clone()),
            layer_version_arn: Some(self.version_arn(version.version)),
            // `Description` is a member of every layer version AWS reports from the
            // version's OWN read: a version published without one carries it as the empty
            // string rather than not at all, exactly as a function configuration does
            // (see [`FunctionRecord::configuration`]), so a client can read the member
            // unconditionally. A LISTING entry is the other way round — see
            // [`LayerRecord::version_summary`].
            description: Some(version.description.clone().unwrap_or_default()),
            created_date: Some(version.created.clone()),
            version: version.version,
            // A list the caller named nothing in is absent from the answer rather than
            // reported empty: the model gives neither member a default, so an unnamed
            // one is a member this version carries no value for.
            compatible_architectures: (!version.compatible_architectures.is_empty())
                .then(|| version.compatible_architectures.clone()),
            compatible_runtimes: (!version.compatible_runtimes.is_empty())
                .then(|| version.compatible_runtimes.clone()),
            license_info: version.license_info.clone(),
        }
    }

    /// The newest version of this layer the store still holds, or `None` once every
    /// version published under the name has been deleted.
    ///
    /// This is NOT [`LayerRecord::latest_version`], which is the highest number ever
    /// published so a number is never handed out twice: `DeleteLayerVersion` takes a
    /// version out of [`LayerRecord::versions`] and leaves that counter where it is, so
    /// the newest version a read can report is the highest one still stored.
    #[must_use]
    pub fn latest_held_version(&self) -> Option<&LayerVersionRecord> {
        self.versions.values().next_back()
    }

    /// The `LayerVersionsListItem` a listing reports for one of this layer's versions,
    /// built entirely out of what is stored.
    #[must_use]
    pub fn version_summary(&self, version: &LayerVersionRecord) -> LayerVersionsListItem {
        LayerVersionsListItem {
            layer_version_arn: Some(self.version_arn(version.version)),
            version: version.version,
            // A LISTING reports only what the version carries: an omitted description is
            // absent from the entry, while the version's own read defaults it to the
            // empty string (see [`LayerRecord::version_detail`]). RECORDED: AWS's
            // `ListLayerVersions` entry for a version published with no optional member
            // at all carries exactly `CreatedDate`, `LayerVersionArn` and `Version`.
            description: version.description.clone(),
            created_date: Some(version.created.clone()),
            // A list the caller named nothing in is absent from the answer rather than
            // reported empty, exactly as [`LayerRecord::version_detail`] reports it.
            compatible_architectures: (!version.compatible_architectures.is_empty())
                .then(|| version.compatible_architectures.clone()),
            compatible_runtimes: (!version.compatible_runtimes.is_empty())
                .then(|| version.compatible_runtimes.clone()),
            license_info: version.license_info.clone(),
        }
    }
}

/// `CapacityProviderState` values this slice moves a capacity provider through, from
/// the model's `CapacityProviderState` enum (`Pending | Active | Failed | Deleting`).
pub const CAPACITY_PROVIDER_STATE_PENDING: &str = "Pending";
/// See [`CAPACITY_PROVIDER_STATE_PENDING`].
pub const CAPACITY_PROVIDER_STATE_ACTIVE: &str = "Active";
/// See [`CAPACITY_PROVIDER_STATE_PENDING`]. `DeleteCapacityProvider` is the one call that
/// moves a provider into it: tearing the compute resources it manages down is owed by the
/// clock, so the record stays visible in this state until the removal is due (see
/// [`CapacityProviderRecord::begin_delete`]).
pub const CAPACITY_PROVIDER_STATE_DELETING: &str = "Deleting";

/// How long after `CreateCapacityProvider` the move from `Pending` to `Active` becomes
/// due.
///
/// An EMULATOR-CHOSEN span (how long Lambda takes to stand the compute resources up is
/// not in the model), deliberately non-zero for the same reason as
/// [`ACTIVATION_DELAY`]: a caller that reads the provider straight after creating it
/// sees the `Pending` state the create answered with. The instant it becomes due is
/// stored on the record and nothing sleeps for it.
pub const CAPACITY_PROVIDER_ACTIVATION_DELAY: Duration = Duration::from_millis(500);

/// How long after `DeleteCapacityProvider` a provider's REMOVAL becomes due.
///
/// An EMULATOR-CHOSEN span (how long Lambda takes to tear the compute resources down is
/// not in the model), deliberately non-zero for the same reason as
/// [`CAPACITY_PROVIDER_ACTIVATION_DELAY`]: a caller that reads the provider straight
/// after deleting it sees the `Deleting` state the delete answered with, and so does an
/// update that arrives while that deletion is in progress. The instant the removal
/// becomes due is stored on the record and nothing sleeps for it (see
/// [`CapacityProviderRecord::begin_delete`]).
pub const CAPACITY_PROVIDER_DELETE_DELAY: Duration = Duration::from_millis(500);

/// The most capacity providers this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and every
/// `CreateCapacityProvider` adds to it, so past this point the slice answers the
/// modelled `CapacityProviderLimitExceededException` ("The maximum number of capacity
/// providers for your account has been exceeded") rather than growing without bound.
pub const MAX_CAPACITY_PROVIDERS: usize = 100;

/// One capacity provider this emulator holds.
///
/// `CreateCapacityProvider` "creates a capacity provider that manages compute resources
/// for Lambda Event Functions", so what it persists is the configuration the request
/// described — the VPC it launches instances in, the role it manages them with, the
/// instance requirements, the scaling configuration, the encryption key, the tag
/// propagation and the telemetry configuration — beside the lifecycle this emulator
/// owns. Every member `GetCapacityProvider` reports comes back out of this record (see
/// [`CapacityProviderRecord::reported`]), so nothing of a request is echoed straight
/// back to its caller.
#[derive(Debug, Clone)]
pub struct CapacityProviderRecord {
    /// The provider's bare name, which is also the key it is stored under.
    pub name: String,
    /// The ARN minted for it once, at creation; see [`capacity_provider_arn`].
    pub arn: String,
    /// The region that ARN carries.
    pub region: String,
    /// `VpcConfig`: the subnets and security groups instances are launched in.
    pub vpc_config: CapacityProviderVpcConfig,
    /// `PermissionsConfig`: the operator role the provider manages compute with.
    pub permissions_config: CapacityProviderPermissionsConfig,
    /// `InstanceRequirements`, when the request stated any.
    pub instance_requirements: Option<InstanceRequirements>,
    /// `CapacityProviderScalingConfig`, when the request stated one.
    pub scaling_config: Option<CapacityProviderScalingConfig>,
    /// `KmsKeyArn`, when the request named a key.
    pub kms_key_arn: Option<String>,
    /// The tags associated with the provider, kept in key order.
    pub tags: BTreeMap<String, String>,
    /// `PropagateTags`, when the request stated it.
    pub propagate_tags: Option<PropagateTags>,
    /// `TelemetryConfig`, when the request stated one.
    pub telemetry_config: Option<CapacityProviderTelemetryConfig>,
    /// The provider's `State`, one of the model's `CapacityProviderState` values.
    pub state: String,
    /// When the move from `Pending` to `Active` becomes due; see
    /// [`CapacityProviderRecord::materialize`].
    pub active_due: SystemTime,
    /// When this provider's REMOVAL becomes due, once a `DeleteCapacityProvider` has
    /// asked for it; `None` for a provider nobody has deleted. See
    /// [`CapacityProviderRecord::begin_delete`].
    pub removal_due: Option<SystemTime>,
    /// `LastModified`: "The date and time when the capacity provider was last
    /// modified", in the same ISO-8601 form as every other timestamp this service
    /// reports.
    pub last_modified: String,
}

impl CapacityProviderRecord {
    /// Bring this record up to whatever the clock has already made due.
    ///
    /// The ONLY transition it applies is the one the clock owed at `now`: a provider
    /// whose compute resources have finished being stood up becomes `Active`. It would
    /// have been owed at the same instant even if nobody had called, so applying it on a
    /// read reports the truth rather than inventing a change.
    pub fn materialize(&mut self, now: SystemTime) {
        if self.state == CAPACITY_PROVIDER_STATE_PENDING && now >= self.active_due {
            self.state = CAPACITY_PROVIDER_STATE_ACTIVE.to_string();
        }
    }

    /// Accept a `DeleteCapacityProvider` of this provider.
    ///
    /// The model's `CapacityProviderState` enum carries `Deleting`, so a provider whose
    /// compute resources are still being torn down is reported in that state: what the
    /// call persists is the state and the instant the removal becomes due
    /// ([`CAPACITY_PROVIDER_DELETE_DELAY`]). The removal itself is owed by the CLOCK and
    /// applied by [`ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`] and
    /// [`resolve_capacity_provider`]; nothing sleeps for it.
    pub fn begin_delete(&mut self, now: SystemTime) {
        self.state = CAPACITY_PROVIDER_STATE_DELETING.to_string();
        self.last_modified = iso8601(now);
        self.removal_due = Some(now + CAPACITY_PROVIDER_DELETE_DELAY);
    }

    /// Whether this provider's deletion is in progress, which is what makes a call that
    /// would change its configuration a conflict with "another operation is in progress"
    /// rather than an accepted change.
    #[must_use]
    pub fn is_deleting(&self) -> bool {
        self.removal_due.is_some()
    }

    /// Whether the clock has already made this provider's removal due; see
    /// [`CapacityProviderRecord::begin_delete`].
    #[must_use]
    pub fn is_removed_by(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// The `CapacityProvider` the wire reports for this record, built entirely out of
    /// what is stored.
    #[must_use]
    pub fn reported(&self) -> CapacityProvider {
        CapacityProvider {
            capacity_provider_arn: self.arn.clone(),
            state: self.state.clone(),
            vpc_config: self.vpc_config.clone(),
            permissions_config: self.permissions_config.clone(),
            instance_requirements: self.instance_requirements.clone(),
            capacity_provider_scaling_config: self.scaling_config.clone(),
            kms_key_arn: self.kms_key_arn.clone(),
            last_modified: Some(self.last_modified.clone()),
            propagate_tags: self.propagate_tags.clone(),
            telemetry_config: self.telemetry_config.clone(),
        }
    }
}

/// The EVENT SOURCE MAPPING slice: the lifecycle an `EventSourceMappingConfiguration`
/// moves through, the reading of the event source an `EventSourceArn` names, the
/// per-source defaults the model documents, and the record itself.
///
/// Held in its own module so this one stays readable; every item is re-exported here, so a
/// handler names them as `crate::state::…` exactly as before. The store that HOLDS the
/// records ([`ComAmazonawsLambdaAwsgirapiserviceState::event_source_mappings`]) stays here
/// beside the function slice, and so do the pollers that read a mapping's source.
pub mod event_source_mapping;
/// The asynchronous-invocation queue: the record one accepted event is held as, the
/// outcomes it moves through, and the delivery that hands it to the function.
///
/// Held in its own module for the same reason as the slice above; every item is
/// re-exported here, so a handler names them as `crate::state::…`.
pub mod async_invocation;
pub use async_invocation::*;
/// How many times a delivery attempt of such an event is MADE — "if you invoke a function
/// asynchronously and it returns an error, Lambda executes the function up to two more
/// times" — lives beside it in [`async_delivery`], the policy that decides what follows one
/// attempt, and is re-exported here for the same reason.
pub mod async_delivery;
pub use async_delivery::*;
pub use event_source_mapping::{
    created_event_source_mapping_state, default_batch_size, default_batching_window_seconds,
    event_source_is_mappable_by_this_account, event_source_mapping_arn, event_source_vendor,
    is_kafka_source, is_shard_source, is_stream_source, metrics_config_refusal,
    mint_event_source_mapping_uuid, names_event_source, queue_visibility_refusal,
    queue_visibility_timeout_seconds, supports_at_timestamp, DEFAULT_BATCHING_WINDOW_SECONDS,
    DEFAULT_BATCH_SIZE_QUEUE, DEFAULT_BATCH_SIZE_STREAM, DEFAULT_PARALLELIZATION_FACTOR,
    DYNAMODB_STREAM_SEGMENT, EVENT_SOURCE_MAPPING_DELETE_DELAY,
    EVENT_SOURCE_MAPPING_DELETE_IN_USE_MESSAGE, EVENT_SOURCE_MAPPING_SETTLE_DELAY,
    EVENT_SOURCE_MAPPING_STATE_CREATING, EVENT_SOURCE_MAPPING_STATE_DELETING,
    EVENT_SOURCE_MAPPING_STATE_DISABLED, EVENT_SOURCE_MAPPING_STATE_ENABLED,
    EVENT_SOURCE_MAPPING_STATE_UPDATING, EVENT_SOURCE_MAPPING_USER_INITIATED,
    EVENT_SOURCE_RESOURCE_BROKER, EVENT_SOURCE_RESOURCE_CLUSTER, EVENT_SOURCE_RESOURCE_STREAM,
    EVENT_SOURCE_RESOURCE_TABLE, EVENT_SOURCE_RESOURCE_VPC_CONNECTION,
    EVENT_SOURCE_VENDOR_DOCUMENT_DB, EVENT_SOURCE_VENDOR_DYNAMODB, EVENT_SOURCE_VENDOR_KAFKA,
    EVENT_SOURCE_VENDOR_KINESIS, EVENT_SOURCE_VENDOR_MQ, EVENT_SOURCE_VENDOR_QUEUE,
    INFINITE_RETENTION, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, LAST_PROCESSING_RESULT_OK,
    MAX_EVENT_SOURCE_MAPPINGS, MAX_EVENT_SOURCE_MAPPING_PAGE_SIZE,
    STARTING_POSITION_AT_TIMESTAMP, STARTING_POSITION_LATEST, STARTING_POSITION_TRIM_HORIZON,
};
pub use event_source_mapping::{EventSourceMappingRecord, ShardCursor, ShardSource};

/// WHICH destinations such a mapping may hand a record of a failed batch to lives in
/// [`event_source_mapping_destination`], beside the slice above, and is re-exported here for
/// the same reason: the two calls that state a mapping's destinations name it as
/// `crate::state::…`.
pub mod event_source_mapping_destination;
pub use event_source_mapping_destination::{
    is_supported_mapping_destination, unsupported_mapping_destination,
    UNSUPPORTED_MAPPING_DESTINATION_MESSAGE,
};

/// Which functions Lambda SnapStart can be applied to, and the refusals for asking for it
/// where it does not hold.
///
/// Held in its own module so this one stays readable; every item is re-exported here, so a
/// handler names them as `crate::state::…`.
pub mod snap_start;
pub use snap_start::*;


/// Per-service state for com-amazonaws-lambda-awsgirapiservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdaAwsgirapiserviceState {
    /// Every function this emulator holds, keyed by function name so `ListFunctions`
    /// pages through them in a stable order.
    pub functions: BTreeMap<String, FunctionRecord>,
    /// Every code-signing configuration, keyed by its ARN — the identifier every
    /// operation on one takes (`GetCodeSigningConfig`, `DeleteCodeSigningConfig`,
    /// `PutFunctionCodeSigningConfig` all bind `CodeSigningConfigArn`).
    pub code_signing_configs: BTreeMap<String, CodeSigningConfigRecord>,
    /// Every capacity provider, keyed by its name — the `{CapacityProviderName}` path
    /// label the capacity-provider operations bind.
    pub capacity_providers: BTreeMap<String, CapacityProviderRecord>,
    /// Every layer this emulator holds, keyed by the REGION it was published into and
    /// its layer name ([`layer_key`]) — a layer is a regional resource, so the same name
    /// in two Regions is two layers — and ordered by that pair, so `ListLayers` pages
    /// through one Region's layers in name order. Reach one through [`Self::layer`],
    /// [`Self::layer_mut`] or [`Self::layers_in`] rather than by keying the map here.
    pub layers: BTreeMap<LayerKey, LayerRecord>,
    /// Every event source mapping this emulator holds, keyed by its UUID so
    /// `ListEventSourceMappings` pages through them in a stable order.
    pub event_source_mappings: BTreeMap<String, EventSourceMappingRecord>,
    /// The UUIDs of the mappings a shard-source poller is already running for, so at most
    /// one runs per mapping; see [`Self::claim_event_source_poller`].
    pub event_source_pollers: BTreeSet<String>,
    /// Whether a poll loop is already running over this store's queue mappings; see
    /// [`sqs_poller::drive_sqs_pollers`], which is the only thing that sets it.
    pub sqs_poller_running: bool,
    /// Which claim on that loop is the current one, so a loop standing down because nothing
    /// polls any more releases only its OWN claim and never one a later call is holding.
    pub sqs_poller_generation: u64,
    /// The running poll loop's task handle, kept so the loop it replaces is aborted rather
    /// than merely forgotten.
    pub sqs_poller: Option<::tokio::task::JoinHandle<()>>,
    /// Whether a concurrency sampler is already running over this account; see
    /// [`concurrency_metering::drive_concurrency_metering`], which is the only thing that
    /// sets it.
    pub concurrency_metering_running: bool,
    /// Which claim on that sampler is the current one, so a sampler standing down because
    /// the account holds no function releases only its OWN claim and never one a later call
    /// is holding.
    pub concurrency_metering_generation: u64,
    /// The running sampler's task handle, kept so the sampler it replaces is aborted rather
    /// than merely forgotten.
    pub concurrency_metering: Option<::tokio::task::JoinHandle<()>>,
    /// The durable-execution status changes waiting to be published onto the account's
    /// default event bus, oldest first and bounded by
    /// [`durable_events::MAX_PENDING_EVENTS`]; see [`durable_events`].
    pub pending_durable_events: VecDeque<durable_events::StatusChange>,
    /// The status last NOTICED for each durable execution, keyed by the ARN the execution is
    /// addressed by, which is what makes the publication a diff: a status that differs from
    /// the one held here is a status the execution moved into. It is written when the change
    /// is QUEUED rather than once the bus has taken it, which is what keeps one move to one
    /// event while the queue is being drained; a batch the bus did not take is retried from
    /// the queue itself ([`durable_events`]). Pruned to the executions the store still holds.
    pub noticed_durable_status: BTreeMap<String, String>,
    /// Whether an event clock is already running over this store's durable executions; see
    /// [`durable_events::publish`], which is the only thing that sets it.
    pub durable_event_clock_running: bool,
    /// Which claim on that clock is the current one, so a clock standing down because nothing
    /// is owed releases only its OWN claim and never one a later call is holding.
    pub durable_event_clock_generation: u64,
    /// The running clock's task handle, kept so the clock it replaces is aborted rather than
    /// merely forgotten.
    pub durable_event_clock: Option<::tokio::task::JoinHandle<()>>,
    /// The secret this store's pagination tokens are authenticated under, drawn from the
    /// system's randomness when the store is created
    /// (`service_common::page_tokens::PageTokenKey`) — which is what lets a paginated read
    /// tell a `Marker` it ISSUED from one a caller assembled; see
    /// [`function_url::function_url_marker`].
    pub page_token_key: PageTokenKey,
}

/// One code-signing configuration.
#[derive(Debug, Clone)]
pub struct CodeSigningConfigRecord {
    /// The `csc-`-prefixed id minted at creation.
    pub id: String,
    /// The configuration's ARN.
    pub arn: String,
    /// The description, when the caller gave one.
    pub description: Option<String>,
    /// The signing profile version ARNs allowed to sign a package.
    pub signing_profile_version_arns: Vec<String>,
    /// `Warn` or `Enforce`, from the model's `CodeSigningPolicy` enum.
    pub untrusted_artifact_on_deployment: String,
    /// ISO-8601 instant of the last change.
    pub last_modified: String,
    /// The tags supplied at creation.
    pub tags: BTreeMap<String, String>,
}

impl CodeSigningConfigRecord {
    /// The `CodeSigningConfig` the wire reports, built entirely out of what is stored.
    #[must_use]
    pub fn to_wire(&self) -> CodeSigningConfig {
        CodeSigningConfig {
            code_signing_config_id: self.id.clone(),
            code_signing_config_arn: self.arn.clone(),
            description: self.description.clone(),
            allowed_publishers: AllowedPublishers {
                signing_profile_version_arns: self.signing_profile_version_arns.clone(),
            },
            code_signing_policies: CodeSigningPolicies {
                untrusted_artifact_on_deployment: Some(
                    self.untrusted_artifact_on_deployment.clone(),
                ),
            },
            last_modified: self.last_modified.clone(),
        }
    }
}

/// One entry of a `ListFunctions` page: the key it is addressed by in the flattened
/// ordering, and the configuration to report.
pub struct ListedFunction {
    /// The pagination key: the function name for `$LATEST`, `name:version` for a
    /// published version.
    pub key: String,
    /// The configuration to report for the entry.
    pub configuration: CreateFunctionOutput,
}

/// One function version that is configured to run on a capacity provider.
///
/// Both surfaces that ask which versions use a provider read the same entries:
/// `DeleteCapacityProvider` refuses a provider that any version still uses, and
/// `ListFunctionVersionsByCapacityProvider` reports those versions — so the relationship
/// is read in ONE place, [`ComAmazonawsLambdaAwsgirapiserviceState::capacity_provider_users`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityProviderVersion {
    /// The ARN of the version: the unqualified function ARN for [`VERSION_LATEST`], and
    /// the `:<version>`-suffixed one for a published version.
    pub function_arn: String,
    /// The name of the function the version belongs to — the first component of the
    /// order these entries are reported in.
    pub function_name: String,
    /// The version label: [`VERSION_LATEST`] or a published version number.
    pub version: String,
    /// The model's `State` of the function this version belongs to.
    pub state: String,
}

impl ComAmazonawsLambdaAwsgirapiserviceState {
    /// Bring every function, every capacity provider and every event source mapping up
    /// to whatever the clock has already made due; see [`FunctionRecord::materialize`],
    /// [`CapacityProviderRecord::materialize`] and
    /// [`EventSourceMappingRecord::materialize`]. A mapping a `DeleteEventSourceMapping`
    /// asked for is removed once that removal is due
    /// ([`EventSourceMappingRecord::begin_delete`]).
    pub fn materialize_all(&mut self, now: SystemTime) {
        for function in self.functions.values_mut() {
            function.materialize(now);
        }
        // A provider whose removal the clock already made due is gone, exactly as a
        // mapping's is below: it would have been gone at the same instant even if nobody
        // had called.
        self.capacity_providers
            .retain(|_, provider| !provider.is_removed_by(now));
        for provider in self.capacity_providers.values_mut() {
            provider.materialize(now);
        }
        // A mapping whose removal the clock already made due is gone: it would have been
        // gone at the same instant even if nobody had called, so dropping it here reports
        // the truth rather than deleting anything a read was not owed.
        self.event_source_mappings
            .retain(|_, mapping| !mapping.is_removed_by(now));
        for mapping in self.event_source_mappings.values_mut() {
            mapping.materialize(now);
        }
    }

    /// Claim the ONE poller of the mapping `uuid` names, reporting whether this caller got
    /// it.
    ///
    /// `false` means a poller is already running for that mapping, so the caller starts
    /// none: an event source is polled for one function by at most one mapping, and a
    /// mapping is polled by at most one poller, so two tasks never read the same shard on
    /// behalf of the same mapping.
    pub fn claim_event_source_poller(&mut self, uuid: &str) -> bool {
        self.event_source_pollers.insert(uuid.to_string())
    }

    /// Release the claim of the poller of the mapping `uuid` names, so a later call can
    /// start one again.
    pub fn release_event_source_poller(&mut self, uuid: &str) {
        self.event_source_pollers.remove(uuid);
    }

    /// Every function version configured to run on the capacity provider `provider_arn`
    /// names, in a stable order.
    ///
    /// A function names the provider it runs on through
    /// `FunctionConfiguration$CapacityProviderConfig`, whose one
    /// `LambdaManagedInstancesCapacityProviderConfig` carries the provider's ARN, and a
    /// published version froze the configuration it was published from — so both the
    /// `$LATEST` configuration and every published version's count as a user of the
    /// provider.
    ///
    /// The order is by function name, `$LATEST` before the published versions of the
    /// same function and those in ascending numeric order, which is the order the store's
    /// maps already yield: it is the order
    /// `ListFunctionVersionsByCapacityProvider` pages through, so it has to be stable
    /// across calls rather than depend on when a version was configured.
    #[must_use]
    pub fn capacity_provider_users(&self, provider_arn: &str) -> Vec<CapacityProviderVersion> {
        let mut users = Vec::new();
        for function in self.functions.values() {
            if configured_capacity_provider(&function.config) == Some(provider_arn) {
                users.push(function.capacity_provider_version(VERSION_LATEST));
            }
            for version in function.versions.values() {
                if configured_capacity_provider(&version.config) == Some(provider_arn) {
                    users.push(function.capacity_provider_version(&version.version.to_string()));
                }
            }
        }
        users
    }

    /// One entry per FUNCTION associated with the capacity provider `provider_arn`
    /// names, in the same stable order as [`Self::capacity_provider_users`], each the ARN
    /// of the association a caller has to remove to free the provider for deletion.
    ///
    /// A function may name one provider from several of its configurations — its
    /// unpublished one and each published version that froze it — and those are ONE
    /// association of ONE function, so a function appears here once however many of its
    /// configurations name the provider. The ARN is the association's own: the qualified
    /// ARN of the first published version that names the provider, and the unqualified
    /// one when only the function's unpublished configuration does.
    ///
    /// It reads the relationship through [`Self::capacity_provider_users`], so the two
    /// cannot disagree about what counts as a use of a provider.
    #[must_use]
    pub fn capacity_provider_user_associations(&self, provider_arn: &str) -> Vec<String> {
        /// One function's association: the function's name, the ARN that names the
        /// association, and whether that ARN is a published version's.
        type Association = (String, String, bool);
        let mut associations: Vec<Association> = Vec::new();
        for user in self.capacity_provider_users(provider_arn) {
            let published = user.version != VERSION_LATEST;
            match associations
                .iter_mut()
                .find(|(function, _, _)| *function == user.function_name)
            {
                // A published version that runs on the provider is what names the
                // association; the unqualified ARN of the unpublished configuration names
                // it only while the function has published no such version.
                Some(held) if published && !held.2 => {
                    held.1 = user.function_arn;
                    held.2 = true;
                }
                Some(_) => {}
                None => associations.push((user.function_name, user.function_arn, published)),
            }
        }
        associations
            .into_iter()
            .map(|(_, arn, _)| arn)
            .collect()
    }

    /// The ARNs of every function associated with the code-signing configuration
    /// `config_arn` names, in a stable order.
    ///
    /// A function names the configuration it deploys under through the single
    /// function-level [`FunctionRecord::code_signing_config_arn`] — the operations that
    /// set and read it take an unqualified `FunctionName`, so a published version is not
    /// a separate user of the configuration and a function appears here at most once,
    /// under its own unqualified ARN.
    ///
    /// It is the one place the association is read from the other side: both
    /// `ListFunctionsByCodeSigningConfig`, which reports these ARNs, and
    /// `DeleteCodeSigningConfig`, which refuses a delete while any of them hold the
    /// configuration, answer from this list.
    #[must_use]
    pub fn code_signing_config_users(&self, config_arn: &str) -> Vec<String> {
        self.functions
            .values()
            .filter(|function| function.code_signing_config_arn.as_deref() == Some(config_arn))
            .map(|function| function.arn.clone())
            .collect()
    }

    /// Every entry `ListFunctions` can report, in pagination order.
    ///
    /// With `include_all_versions` each function's published versions are listed
    /// beside its `$LATEST` entry, which is what `FunctionVersion=ALL` asks for ("Set
    /// to ALL to include entries for all published versions of each function").
    ///
    /// The result is ordered BY ITS PAGINATION KEY, not by the order the entries are
    /// generated in. `ListFunctions` continues a page by taking the first entry whose
    /// key sorts after the caller's `Marker`, so the sequence has to be sorted the same
    /// way or an entry could be skipped: a version key (`name:1`) does not sort
    /// directly after its function's own key when another function's name extends that
    /// name (`fn-1`, `fn-1:1`, `fn-10`), because `:` sorts after a digit.
    #[must_use]
    pub fn listed_functions(&self, include_all_versions: bool) -> Vec<ListedFunction> {
        let mut entries = Vec::new();
        for function in self.functions.values() {
            entries.push(ListedFunction {
                key: function.config.name.clone(),
                configuration: listed_configuration(
                    function.configuration(VERSION_LATEST, &function.config),
                ),
            });
            if include_all_versions {
                for published in function.versions.values() {
                    let label = published.version.to_string();
                    entries.push(ListedFunction {
                        key: format!("{}:{label}", function.config.name),
                        configuration: listed_configuration(
                            function.configuration(&label, &published.config),
                        ),
                    });
                }
            }
        }
        entries.sort_by(|left, right| left.key.cmp(&right.key));
        entries
    }

    /// `AccountUsage`: "The number of functions and amount of storage in use", computed
    /// from the functions this store holds.
    ///
    /// `FunctionCount` is "The number of Lambda functions", so it counts functions rather
    /// than their versions. `TotalCodeSize` is "The amount of storage space, in bytes,
    /// that's being used by deployment packages and layer archives": each published
    /// version keeps its own package, and AWS counts every one of them against the code
    /// storage quota, so the sum covers `$LATEST` and each published version. Every
    /// published layer version's archive is the other term that sentence names.
    #[must_use]
    pub fn account_usage(&self) -> AccountUsage {
        let package_code_size = self
            .functions
            .values()
            .flat_map(|function| {
                std::iter::once(function.config.code_size).chain(
                    function
                        .versions
                        .values()
                        .map(|published| published.config.code_size),
                )
            })
            .fold(0i64, i64::saturating_add);
        let archive_code_size = self
            .layers
            .values()
            .flat_map(|layer| layer.versions.values().map(|version| version.code_size))
            .fold(0i64, i64::saturating_add);
        let total_code_size = package_code_size.saturating_add(archive_code_size);
        AccountUsage {
            total_code_size,
            function_count: i64::try_from(self.functions.len()).unwrap_or(i64::MAX),
        }
    }

    /// The concurrency the stored functions reserve, leaving out the one function
    /// `exclude` names when it names one.
    ///
    /// `PutFunctionConcurrency` REPLACES the reservation of the function it addresses, so
    /// a caller checking what a new reservation would leave the account asks for the sum
    /// without that function's current one.
    #[must_use]
    pub fn reserved_concurrency_except(&self, exclude: Option<&str>) -> i32 {
        self.functions
            .iter()
            .filter(|(name, _)| exclude.is_none_or(|excluded| name.as_str() != excluded))
            .filter_map(|(_, function)| function.reserved_concurrent_executions)
            .fold(0i32, i32::saturating_add)
    }

    /// The provisioned concurrency the stored functions have allocated out of the
    /// account's UNRESERVED capacity, leaving out what `exclude` names when it names
    /// something: a function name beside one of its qualifiers leaves out that one
    /// configuration, and a function name beside `None` leaves out every configuration of
    /// that function.
    ///
    /// A function that reserves concurrency allocates its provisioned concurrency out of
    /// that reservation — `PutFunctionConcurrency` "reserves capacity for that
    /// concurrency level", and the reservation is the whole of what the account commits
    /// to that function — so only the functions reserving nothing commit anything here
    /// beyond what [`Self::reserved_concurrency_except`] already counts. That is also why
    /// a caller weighing a NEW reservation for a function leaves out the whole of that
    /// function's provisioned concurrency: the reservation it is weighing is what that
    /// capacity would come out of from then on.
    #[must_use]
    pub fn unreserved_provisioned_concurrency_except(
        &self,
        exclude: Option<(&str, Option<&str>)>,
    ) -> i32 {
        self.functions
            .iter()
            .filter(|(_, function)| function.reserved_concurrent_executions.is_none())
            .filter_map(|(name, function)| {
                let excluded = exclude.filter(|(excluded_name, _)| *excluded_name == name.as_str());
                match excluded {
                    // Every configuration of this function is left out.
                    Some((_, None)) => None,
                    Some((_, Some(qualifier))) => {
                        Some(function.provisioned_concurrency_except(Some(qualifier)))
                    }
                    None => Some(function.provisioned_concurrency_except(None)),
                }
            })
            .fold(0i32, i32::saturating_add)
    }

    /// The concurrency this account has ALLOCATED to individual functions: what every
    /// function reserves, plus the provisioned concurrency allocated on the functions that
    /// reserve none.
    ///
    /// This is the one quantity the account's unreserved capacity is measured against, so
    /// [`Self::account_limit`] reports the limit less this, and an operation that commits
    /// more capacity weighs its documented floor against the same sum with its own
    /// allocation left out.
    #[must_use]
    pub fn allocated_concurrency(&self) -> i32 {
        self.reserved_concurrency_except(None)
            .saturating_add(self.unreserved_provisioned_concurrency_except(None))
    }

    /// Whether any function this account holds is configured with a runtime Lambda no
    /// longer supports.
    ///
    /// A function runs the runtime its configuration names, and a published version keeps
    /// the runtime it froze and still runs when it is invoked, so a deprecated runtime
    /// anywhere among them — the unpublished configuration or any published version — is
    /// one this account still has a function on. A function whose package type is an
    /// image names no runtime at all, so it holds none to be deprecated.
    #[must_use]
    pub fn has_function_with_deprecated_runtime(&self) -> bool {
        self.functions.values().any(|function| {
            std::iter::once(&function.config)
                .chain(function.versions.values().map(|version| &version.config))
                .filter_map(|config| config.runtime.as_deref())
                .any(is_deprecated_runtime)
        })
    }

    /// `AccountLimit`: the account's quotas, with the one limit that is not a constant
    /// computed from the store.
    ///
    /// `UnreservedConcurrentExecutions` is modelled as "The maximum number of
    /// simultaneous function executions, minus the capacity that's reserved for
    /// individual functions with PutFunctionConcurrency", so it is the account's
    /// concurrency limit less every allocation the account has committed to an individual
    /// function — each reservation, and the provisioned concurrency of the functions that
    /// reserve nothing, which is capacity kept initialised for one alias or version and so
    /// is no longer available to a function without a per-function limit (see
    /// [`Self::allocated_concurrency`]). The member is `@range(min: 0)`, so the
    /// subtraction is floored at zero.
    #[must_use]
    pub fn account_limit(&self) -> AccountLimit {
        let allocated = self.allocated_concurrency();
        AccountLimit {
            total_code_size: TOTAL_CODE_SIZE_LIMIT,
            code_size_unzipped: CODE_SIZE_UNZIPPED_LIMIT,
            code_size_zipped: CODE_SIZE_ZIPPED_LIMIT,
            concurrent_executions: CONCURRENT_EXECUTIONS_LIMIT,
            unreserved_concurrent_executions: Some(
                CONCURRENT_EXECUTIONS_LIMIT.saturating_sub(allocated).max(0),
            ),
        }
    }
}

/// Drop the members `ListFunctions` does not report.
///
/// The model's own `ListFunctions` documentation states which they are: "The
/// ListFunctions operation returns a subset of the FunctionConfiguration fields. To get
/// the additional fields (State, StateReasonCode, StateReason, LastUpdateStatus,
/// LastUpdateStatusReason, LastUpdateStatusReasonCode, RuntimeVersionConfig) for a
/// function or version, use GetFunction."
fn listed_configuration(mut configuration: CreateFunctionOutput) -> CreateFunctionOutput {
    configuration.state = None;
    configuration.state_reason = None;
    configuration.state_reason_code = None;
    configuration.last_update_status = None;
    configuration.last_update_status_reason = None;
    configuration.last_update_status_reason_code = None;
    configuration.runtime_version_config = None;
    configuration
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLambdaAwsgirapiserviceState>)
}

/// The unqualified ARN of a function:
/// `arn:aws:lambda:<region>:<account>:function:<name>`.
#[must_use]
pub fn function_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("function:{name}"))
}

/// The ARN resource type a code-signing configuration carries, from the model's
/// `CodeSigningConfigArn` pattern (`…:code-signing-config:csc-[a-z0-9]{17}`). The ARN is
/// minted from it and the tagging surface reads it back by it, so the two cannot drift.
pub const CODE_SIGNING_CONFIG_RESOURCE_TYPE: &str = "code-signing-config";

/// The ARN of a code-signing configuration:
/// `arn:aws:lambda:<region>:<account>:code-signing-config:<id>`, as the model's
/// `CodeSigningConfigArn` pattern requires.
#[must_use]
pub fn code_signing_config_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("{CODE_SIGNING_CONFIG_RESOURCE_TYPE}:{id}"),
    )
}

/// A fresh code-signing configuration id.
///
/// The model's `CodeSigningConfigId` pattern is `^csc-[a-zA-Z0-9-_\.]{17}$` and its
/// `CodeSigningConfigArn` pattern narrows the same id to `csc-[a-z0-9]{17}`, so the 17
/// characters are taken from the lower-case hexadecimal of a fresh UUID: unique per
/// configuration and inside both patterns.
#[must_use]
pub fn mint_code_signing_config_id() -> String {
    let hex = ::uuid::Uuid::new_v4().simple().to_string();
    format!("csc-{}", &hex[..17])
}

/// The default CloudWatch log group of a function: "By default, Lambda functions send
/// logs to a default log group named `/aws/lambda/<function name>`" (the model's own
/// `LoggingConfig$LogGroup` documentation).
#[must_use]
pub fn default_log_group(name: &str) -> String {
    format!("/aws/lambda/{name}")
}

/// The ARN of the managed runtime version a function is pinned to:
/// `arn:aws:lambda:<region>::runtime:<id>`, the first form the model's
/// `RuntimeVersionArn` pattern admits.
///
/// The id identifies the runtime BUILD, so it is DERIVED from the runtime identifier
/// through `service_common::stable_hex_id`: two reads of a function on one runtime report
/// the same runtime version, two runtimes report different ones, and nothing is taken
/// from the clock or from the request.
#[must_use]
pub fn runtime_version_arn(region: &str, runtime: &str) -> String {
    format!(
        "arn:aws:lambda:{region}::runtime:{}",
        ::service_common::stable_hex_id(&[runtime])
    )
}

/// The unqualified ARN of a layer: `arn:aws:lambda:<region>:<account>:layer:<name>`,
/// as the model's `LayerArn` pattern requires.
#[must_use]
pub fn layer_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("layer:{name}"))
}

/// The capacity provider a function configuration runs on, when it names one.
///
/// `CapacityProviderConfig` carries exactly one
/// `LambdaManagedInstancesCapacityProviderConfig`, whose required `CapacityProviderArn`
/// is the reference, so this is the one place that reads it.
#[must_use]
pub fn configured_capacity_provider(config: &FunctionConfig) -> Option<&str> {
    config.capacity_provider_config.as_ref().map(|capacity| {
        capacity
            .lambda_managed_instances_capacity_provider_config
            .capacity_provider_arn
            .as_str()
    })
}

/// Resolve a `CapacityProviderName` (in either of its documented forms) to one stored
/// capacity provider.
///
/// The record is brought up to whatever the clock already made due before it is handed
/// back, so every caller reports the same lifecycle state at the same instant.
pub fn resolve_capacity_provider<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    capacity_provider_name: &str,
    region: &str,
    now: SystemTime,
) -> Result<&'a mut CapacityProviderRecord, ResolveFailure> {
    let name = parse_capacity_provider_name(capacity_provider_name).ok_or_else(|| {
        ResolveFailure::Malformed(capacity_provider_name_message(capacity_provider_name))
    })?;
    // A provider whose removal the clock already made due is gone before this lookup
    // sees it, so addressing it is addressing a provider this account no longer holds.
    store
        .capacity_providers
        .retain(|_, provider| !provider.is_removed_by(now));
    let Some(record) = store.capacity_providers.get_mut(&name) else {
        return Err(ResolveFailure::NotFound(format!(
            "Capacity provider not found: {}. Check that the ARN is correct and the capacity \
             provider exists in this Region.",
            capacity_provider_arn(region, &name)
        )));
    };
    record.materialize(now);
    Ok(record)
}

/// Whether `name` is a layer name, which the model's `LayerName` pattern spells
/// `[a-zA-Z0-9-_]+`.
#[must_use]
pub fn is_layer_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// What a `LayerVersionArn` names.
///
/// The model's pattern admits exactly two forms, and they name resources in two
/// different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerVersionReference {
    /// `arn:<partition>:lambda:<region>:<account>:layer:<name>:<version>` — a version
    /// published into an account, which this store holds.
    Published {
        /// The Region the ARN names, which is the Region the version was published into:
        /// a layer is a regional resource, so this is part of WHICH version is named.
        region: String,
        /// The layer's bare name.
        name: String,
        /// The version number the ARN qualifies it with.
        version: i64,
    },
    /// `arn:<partition>:lambda:::awslayer:<name>` — a layer Lambda itself provides,
    /// which belongs to no account and is held in no store.
    AwsManaged,
}

/// Split a `LayerVersionArn` into the parts that identify a layer version, or `None`
/// when the value is neither documented form.
#[must_use]
pub fn parse_layer_version_arn(value: &str) -> Option<LayerVersionReference> {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.first() != Some(&"arn") || parts.get(2) != Some(&ARN_NAMESPACE) {
        return None;
    }
    let resource_type = parts.get(5).copied().unwrap_or_default();
    match (parts.len(), resource_type) {
        (8, "layer") if is_layer_name(parts[6]) => {
            let version = parts[7].parse::<i64>().ok().filter(|version| *version > 0)?;
            Some(LayerVersionReference::Published {
                region: parts[3].to_string(),
                name: parts[6].to_string(),
                version,
            })
        }
        (7, "awslayer") if is_layer_name(parts[6]) => Some(LayerVersionReference::AwsManaged),
        _ => None,
    }
}

/// The region this request's ARNs carry: the one the caller signed for, from the SigV4
/// credential scope, falling back to [`DEFAULT_REGION`] for an unsigned request.
#[must_use]
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The deployment package bytes this emulator holds for `code`, or `None` when it holds
/// none (an S3 object or a container image it does not resolve).
#[must_use]
pub fn held_package_bytes(code: &CodeRecord) -> Option<&[u8]> {
    code.zip_bytes.as_deref()
}

/// The digest-pinned URI `FunctionCodeLocation$ResolvedImageUri` reports for a container
/// image.
///
/// The member is "The resolved URI for the image", and what AWS resolves a mutable tag to
/// is the same repository pinned to the CONTENT behind it (`<repository>@sha256:<digest>`),
/// which is what makes the resolved URI say which image the function is frozen to — and
/// what a client compares against `DescribeImages`/`BatchGetImage`. So the digest is the
/// one the REGISTRY minted for the reference, which the call that stored the package
/// resolved and kept on the record (`resolved_digest`, see [`image_source`]).
///
/// A package stored while no registry could be asked carries none, and nothing the
/// registry minted can be reported for it: the resolved URI then pins the repository to a
/// digest over the reference itself, which is stable across reads, equal for two functions
/// on the same image and different for two tags of one repository.
#[must_use]
pub fn resolved_image_uri(image_uri: &str, resolved_digest: Option<&str>) -> String {
    let repository = image_repository(image_uri);
    if let Some(digest) = resolved_digest {
        return format!("{repository}@{digest}");
    }
    format!("{repository}@sha256:{}", image_code_sha256(image_uri))
}

/// The digest of the container image an `Image` package names, for a package stored while
/// no registry could be asked: the content this emulator holds for such a package is the
/// reference itself, so the digest is stable across reads, equal for two functions on the
/// same image and different for two references.
///
/// AWS reports an image function's `CodeSha256` as the digest of the image it is frozen to,
/// rendered as the lower-case hexadecimal an OCI digest is written in — unlike a .zip
/// archive's, which is base64 ([`code_sha256`]) — so the two are told apart by their form
/// as AWS tells them apart. A package whose image the REGISTRY resolved reports that
/// registry's own digest instead (see [`package_code::reported_code_sha256`]).
#[must_use]
pub fn image_code_sha256(image_uri: &str) -> String {
    let digest = sha2::Sha256::digest(image_uri.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The repository part of an image URI: everything before the tag or the digest that
/// qualifies it.
///
/// A tag is separated by the LAST `:` after the final `/`, so a registry carrying a port
/// (`<host>:<port>/<repository>`) is not read as a tag.
fn image_repository(image_uri: &str) -> &str {
    if let Some((repository, _)) = image_uri.split_once('@') {
        return repository;
    }
    match image_uri.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => repository,
        _ => image_uri,
    }
}

/// `RetentionPeriodInDays` of a durable-execution configuration that never set one.
///
/// The member is "The number of days to retain execution history after a durable
/// execution completes", and a configuration always has an effective retention, so a
/// read reports this one until a caller states another. RECORDED from AWS's own answer to
/// a `CreateFunction` carrying a `DurableConfig` that omits the member; it is inside the
/// member's modelled range (1..90).
pub const DEFAULT_DURABLE_RETENTION_PERIOD_DAYS: i32 = 14;

/// The durable-execution configuration a function holds once `requested` is applied to
/// `previous`.
///
/// `DurableConfig` is a configuration rather than a value: a request that names it
/// changes the members it names and leaves the rest of the configuration where it was,
/// and a member nobody has ever set carries its documented default
/// ([`DEFAULT_DURABLE_RETENTION_PERIOD_DAYS`]). A request that does not name it at all
/// leaves the configuration untouched — including absent, for a function that is not
/// configured for durability ("The function's durable execution configuration settings,
/// if the function is configured for durability").
///
/// What is RECORDED is the default and a partial update that reads both members back
/// afterwards; no recorded answer shows a member being CLEARED by omission, so an update
/// changes what it names rather than replacing the whole configuration.
#[must_use]
pub fn effective_durable_config(
    previous: Option<&DurableConfig>,
    requested: Option<&DurableConfig>,
) -> Option<DurableConfig> {
    let Some(requested) = requested else {
        return previous.cloned();
    };
    Some(DurableConfig {
        kms_key_arn: requested
            .kms_key_arn
            .clone()
            .or_else(|| previous.and_then(|held| held.kms_key_arn.clone())),
        retention_period_in_days: requested
            .retention_period_in_days
            .or_else(|| previous.and_then(|held| held.retention_period_in_days))
            .or(Some(DEFAULT_DURABLE_RETENTION_PERIOD_DAYS)),
        execution_timeout: requested
            .execution_timeout
            .or_else(|| previous.and_then(|held| held.execution_timeout)),
    })
}

/// The service an event source belongs to, as AWS NAMES it in the duplicate-mapping
/// refusal below.
///
/// One row per vendor an `EventSourceArn` can carry. The SQS row is RECORDED from AWS's
/// own answer to a second `CreateEventSourceMapping` of one queue-and-function pair; the
/// others state the service's own name, because no answer for them is recorded. A vendor
/// with no row is named by the ARN's service field as it stands.
const EVENT_SOURCE_LABELS: &[(&str, &str)] = &[
    (EVENT_SOURCE_VENDOR_QUEUE, "SQS"),
    (EVENT_SOURCE_VENDOR_KINESIS, "Kinesis"),
    (EVENT_SOURCE_VENDOR_DYNAMODB, "DynamoDB"),
    (EVENT_SOURCE_VENDOR_KAFKA, "MSK"),
    (EVENT_SOURCE_VENDOR_MQ, "MQ"),
    (EVENT_SOURCE_VENDOR_DOCUMENT_DB, "DocumentDB"),
];

/// How the refusal below names the service `vendor` belongs to.
fn event_source_label(vendor: Option<&str>) -> String {
    let vendor = vendor.unwrap_or_default();
    EVENT_SOURCE_LABELS
        .iter()
        .find(|(candidate, _)| *candidate == vendor)
        .map_or_else(|| vendor.to_string(), |(_, label)| (*label).to_string())
}

/// The refusal a `CreateEventSourceMapping` of a pair that already has a mapping earns.
///
/// The message is built from the mapping that already exists — the event source it
/// polls, the function it invokes and the UUID a caller updates or deletes it by — so a
/// client holding many mappings can tell WHICH pair collided from the answer alone.
#[must_use]
pub fn duplicate_event_source_mapping_message(
    event_source_arn: &str,
    function_name: &str,
    uuid: &str,
) -> String {
    format!(
        "An event source mapping with {label} arn (\" {event_source_arn} \") and function \
         (\" {function_name} \") already exists. Please update or delete the existing mapping \
         with UUID {uuid}",
        label = event_source_label(event_source_vendor(event_source_arn)),
    )
}

/// WHICH RECORDS a mapping reads — the identity a second `CreateEventSourceMapping` for
/// one function is judged a duplicate by.
///
/// "Creates a mapping between an event source and an Lambda Event Function": the source
/// is either the resource `EventSourceArn` names or a self-managed Apache Kafka cluster,
/// which has no ARN and is named by its brokers instead ("The list of bootstrap servers
/// for your Kafka brokers"). Inside either, a mapping on a Kafka source consumes the one
/// `Topics` entry and a mapping on an Amazon MQ broker the one `Queues` entry the request
/// named ("The name of the Kafka topic", "The name of the Amazon MQ broker destination
/// queue to consume"), and a source with neither is consumed whole. So the identity is
/// the source TOGETHER WITH what is consumed inside it: two mappings of one cluster
/// reading different topics read different records and are different mappings, while two
/// that agree on all of it would hand one function the same records twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolledEventSource {
    /// The source, as the refusal names it: the `EventSourceArn`, or the bootstrap
    /// servers of the self-managed cluster.
    source: String,
    /// Whether [`Self::source`] is an ARN, which the two refusal wordings differ on.
    named_by_arn: bool,
    /// The `Topics` consumed inside the source, in a fixed order.
    topics: Vec<String>,
    /// The `Queues` consumed inside the source, in a fixed order.
    queues: Vec<String>,
}

/// What a mapping with these members polls, or `None` when they name no source at all —
/// nothing another mapping could collide with.
///
/// The endpoints and the consumed topics and queues are put in a FIXED order, because
/// `Endpoints` arrives as a map and the two lists as lists: one cluster and one topic
/// have to compare equal however the request happened to order them, and the same order
/// is what the refusal names them in.
#[must_use]
pub fn polled_event_source(
    event_source_arn: Option<&str>,
    self_managed_event_source: Option<&SelfManagedEventSource>,
    topics: Option<&[String]>,
    queues: Option<&[String]>,
) -> Option<PolledEventSource> {
    let ordered = |values: &[String]| {
        let mut values = values.to_vec();
        values.sort();
        values.dedup();
        values
    };
    let (source, named_by_arn) = match event_source_arn.filter(|arn| !arn.is_empty()) {
        Some(event_source_arn) => (event_source_arn.to_string(), true),
        None => {
            // `Endpoints` maps each `EndPointType` to the servers of that type, so the
            // cluster is named by every server under every type it carries.
            let servers: Vec<String> = self_managed_event_source
                .and_then(|self_managed| self_managed.endpoints.as_ref())
                .map(|endpoints| endpoints.values().flatten().cloned().collect())
                .unwrap_or_default();
            let servers = ordered(&servers);
            if servers.is_empty() {
                return None;
            }
            (servers.join(","), false)
        }
    };
    Some(PolledEventSource {
        source,
        named_by_arn,
        topics: ordered(topics.unwrap_or_default()),
        queues: ordered(queues.unwrap_or_default()),
    })
}

impl PolledEventSource {
    /// The refusal a second `CreateEventSourceMapping` of this source for one function
    /// earns, built from the mapping that already exists so a caller can converge on it.
    ///
    /// The source and what is consumed inside it come from the request; the function and
    /// the UUID are the existing mapping's, which is the one a caller updates or deletes
    /// to resolve the collision. A source consumed whole and named by an ARN is reported
    /// by [`duplicate_event_source_mapping_message`] — the wording RECORDED for it —
    /// while a source that is read per topic or per queue, or that has no ARN to name, is
    /// reported with the source, the function and the consumed member named in turn.
    #[must_use]
    pub fn duplicate_message(
        &self,
        function_arn: &str,
        function_name: &str,
        uuid: &str,
    ) -> String {
        let consumed = [("topics", &self.topics), ("queues", &self.queues)]
            .into_iter()
            .find(|(_, values)| !values.is_empty())
            .map(|(member, values)| format!(", {member} (\"{}\")", values.join(",")));
        if consumed.is_none() && self.named_by_arn {
            return duplicate_event_source_mapping_message(&self.source, function_name, uuid);
        }
        format!(
            "An event source mapping with event source (\"{source}\"), function \
             (\"{function_arn}\"){consumed} already exists. Please update or delete the \
             existing mapping with UUID {uuid}",
            source = self.source,
            consumed = consumed.unwrap_or_default(),
        )
    }
}

/// What a call on a resource-based policy answers when there is no statement for it to
/// act on: a REMOVAL always, and a READ of a layer version that has no policy record at
/// all — one nobody has ever granted anything on.
///
/// It is the POLICY that is reported missing rather than the resource, so a removal
/// addressed at a function or a layer version that exists but carries no policy answers
/// this rather than [`RESOURCE_NOT_FOUND_MESSAGE`] — which is what `GetPolicy` answers,
/// because a read reports on the resource it was asked about.
pub const NO_POLICY_MESSAGE: &str = "No policy is associated with the given resource.";

/// What a REMOVAL answers when the policy it is to act on exists but holds no statement
/// under the id the request named.
///
/// The policy is there, so what is missing is the one statement the request addressed:
/// the sentence names that statement id — the only part of the request it carries — and
/// states where it was looked for, which is the resource policy the request addressed
/// rather than any resource this service could name back. It is therefore the same
/// sentence for a function's policy and a layer version's, since neither the statement id
/// nor "the resource policy" is a property of the resource kind, while
/// [`NO_POLICY_MESSAGE`] is what a removal gets when there is no policy to act on at all.
#[must_use]
pub fn statement_not_found_message(statement_id: &str) -> String {
    format!("Statement {statement_id} is not found in resource policy.")
}

/// What a READ of a layer version answers when its policy record holds no statement:
/// every statement it had has been removed, so the version it names is the one that has
/// no policy left.
///
/// The layer version EXISTS in both this case and [`NO_POLICY_MESSAGE`]'s, so neither is
/// the refusal for a layer version this account does not hold.
#[must_use]
pub fn layer_version_without_policy_message(layer_version_arn: &str) -> String {
    format!("Layer version {layer_version_arn} does not have any resource policy.")
}

/// The time-zone designator every ISO-8601 instant this service reports carries: the
/// NUMERIC UTC offset, not the `Z` designator.
///
/// Which of the two ISO-8601 designators Lambda writes is in the model's own example of
/// such a member — `LayerVersionsListItem$CreatedDate` is documented as "in ISO 8601
/// format. For example, `2018-11-27T15:10:45.123+0000`"
/// (`smithy_query(query="doc", shape="LayerVersionsListItem", member="CreatedDate")`) —
/// and every other instant member of this service is documented by the same phrase
/// ("in ISO-8601 format (YYYY-MM-DDThh:mm:ss.sTZD)"), whose `TZD` is that designator. So
/// one rendering serves them all, and a caller parsing any of them with the offset form
/// reads every one of them.
pub const UTC_DESIGNATOR: &str = "+0000";

/// `at` as the ISO-8601 instant the model's `LastModified` member describes
/// ("in ISO-8601 format (YYYY-MM-DDThh:mm:ss.sTZD)"), with the designator
/// [`UTC_DESIGNATOR`] spells.
///
/// The subsecond field is TRUNCATED to whole milliseconds and rendered at a fixed three
/// digits, so two instants this service reports always compare the same way as strings
/// as they do as instants — two instants inside one millisecond render as the same
/// string, so the order is not strict, but it is never reversed. The `DateTime`
/// formatter trims trailing zeros instead, which makes one rendering a PREFIX of another
/// (`…:43.848` against `…:43.848766`) and so reverses their order under a string
/// comparison, and a caller reading `LastModified` of one resource twice, or of two
/// resources, has only the string to order them by.
#[must_use]
pub fn iso8601(at: SystemTime) -> String {
    let instant = ::aws_smithy_types::DateTime::from(at);
    let seconds = ::aws_smithy_types::DateTime::from_secs(instant.secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    let millis = instant.subsec_nanos() / 1_000_000;
    format!(
        "{}.{millis:03}{UTC_DESIGNATOR}",
        seconds.trim_end_matches('Z')
    )
}

/// The base64-encoded SHA-256 of `bytes`, which is what the model's `CodeSha256`
/// member reports ("The SHA256 hash of the function's deployment package").
#[must_use]
pub fn code_sha256(bytes: &[u8]) -> String {
    let digest = sha2::Sha256::digest(bytes);
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// The `CodeSize` and `CodeSha256` of the package `code` names — the ONE reading both
/// operations that store a package report them from, so a function's reported code identity
/// does not depend on which call stored it.
///
/// `CodeSha256` is "The SHA256 hash of the function's deployment package", which AWS reports
/// for every package type: for a .zip archive it is the digest of the archive's bytes
/// (base64, as [`code_sha256`] renders it), and for a container image it is the digest of
/// the IMAGE the function is frozen to — the same digest `ResolvedImageUri` pins that image
/// by ([`image_code_sha256`]), so the two members a caller compares deployed image content
/// with cannot disagree.
///
/// `CodeSize` is "The size of the function's deployment package, in bytes", which can only
/// be measured for the bytes this emulator HOLDS — for a package named by an Amazon S3
/// location, the ones Amazon S3 handed over when the package was stored
/// ([`s3_source`]); an object nothing could be shown about has no size it measured and no
/// digest it computed, so it reports neither.
#[must_use]
pub fn package_code_identity(code: &CodeRecord) -> (i64, Option<String>) {
    if let Some(bytes) = &code.zip_bytes {
        return (
            i64::try_from(bytes.len()).unwrap_or(i64::MAX),
            Some(code_sha256(bytes)),
        );
    }
    // A container image is a reference this emulator does not pull, so it holds no archive
    // to measure — and the digest it is frozen to is the manifest digest the REGISTRY
    // minted for that reference, which the call that stored the package resolved (see
    // [`package_code::reported_code_sha256`]); a package stored while no registry could be
    // asked falls back to the digest of the reference itself ([`image_code_sha256`]).
    if code.image_uri.is_some() {
        return (0, reported_code_sha256(code));
    }
    (0, None)
}

/// Lambda's sentence for a `CodeSha256` precondition that did not hold.
///
/// `PublishVersion` takes "Only publish a version if the hash value matches the value
/// that's specified", and the refusal reports BOTH digests — the one the caller supplied
/// and the one the unpublished version currently carries — under the member's own
/// `CodeSHA256` spelling, and tells the caller to retry with the current one. Both values
/// come from the request and from stored state.
#[must_use]
pub fn code_sha256_precondition_message(supplied: &str, current: &str) -> String {
    format!(
        "CodeSHA256 ({supplied}) is different from current CodeSHA256 in {VERSION_LATEST} \
         ({current}). Please try again with the CodeSHA256 in {VERSION_LATEST}."
    )
}

/// The refusal the same precondition gets when the function's package is one this
/// emulator holds no bytes for (an S3 object or a container image), so no `CodeSha256` was
/// ever computed for it.
///
/// AWS always holds a digest, so it has no sentence for this state and
/// [`code_sha256_precondition_message`]'s would have an empty slot where the current
/// digest goes. This says instead what is true: there is no digest to compare against, so
/// nothing is reported for the function that was not computed.
#[must_use]
pub fn code_sha256_without_package_message(supplied: &str) -> String {
    format!(
        "CodeSHA256 ({supplied}) cannot be matched: this function's deployment package is \
         held by reference, so no CodeSHA256 is computed for {VERSION_LATEST}."
    )
}

/// A fresh revision id. Lambda's `RevisionId` is a UUID, and a new one is minted on
/// every change to a function's code or configuration.
#[must_use]
pub fn mint_revision_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// AWS's published Lambda quota for environment variables: 4 KB for all of a function's
/// environment variables combined.
pub const MAX_ENVIRONMENT_BYTES: usize = 4 * 1024;

/// The environment variable names Lambda sets itself and will not let a function's
/// configuration set.
///
/// The Lambda Developer Guide's "Use Lambda environment variables" lists these as the
/// RESERVED runtime environment variables: "Lambda reserves [them] … you cannot set these
/// keys in your function's configuration." The variables the runtime sets that the guide
/// does NOT reserve (`TZ`, `LANG`, `PATH`, `LD_LIBRARY_PATH`,
/// `AWS_XRAY_CONTEXT_MISSING`, the X-Ray daemon address and port) are deliberately absent:
/// a function may set those, and the run's own environment applies its value
/// (`crate::state::execution`). A request naming a reserved key is refused whole, so the
/// function keeps the environment it had.
const RESERVED_ENVIRONMENT_KEYS: &[&str] = &[
    "AWS_ACCESS_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_DEFAULT_REGION",
    "AWS_EXECUTION_ENV",
    "AWS_LAMBDA_FUNCTION_MEMORY_SIZE",
    "AWS_LAMBDA_FUNCTION_NAME",
    "AWS_LAMBDA_FUNCTION_VERSION",
    "AWS_LAMBDA_INITIALIZATION_TYPE",
    "AWS_LAMBDA_LOG_GROUP_NAME",
    "AWS_LAMBDA_LOG_STREAM_NAME",
    "AWS_LAMBDA_RUNTIME_API",
    "AWS_REGION",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SECRET_KEY",
    "AWS_SESSION_TOKEN",
    "LAMBDA_RUNTIME_DIR",
    "LAMBDA_TASK_ROOT",
    "_HANDLER",
    "_X_AMZN_TRACE_ID",
];

/// What the environment budget is measured on: the variables as ONE serialized JSON
/// object, which is the string AWS's refusal reports it measured.
///
/// The quota is on the set rather than on any one variable, so what counts is the whole
/// mapping as it travels — every key, every value and the punctuation that separates them
/// — and not the bare key and value bytes, which would admit a set AWS refuses. Written
/// through `serde_json`, so a character a JSON string cannot hold literally contributes
/// the bytes of its escape, exactly as it does on the wire.
#[must_use]
pub fn measured_environment(variables: &BTreeMap<String, String>) -> String {
    ::serde_json::to_string(variables).unwrap_or_default()
}

/// The refusal a set of environment variables past [`MAX_ENVIRONMENT_BYTES`] gets.
///
/// Fixed text, with the string the quota was measured on as its one variable part, which
/// is what tells a caller what was counted.
#[must_use]
pub fn environment_too_large_message(measured: &str) -> String {
    format!(
        "Lambda was unable to configure your environment variables because the environment \
         variables you have provided exceeded the 4KB limit. String measured: {measured}"
    )
}

/// The refusal a set of environment variables naming a key Lambda reserves gets.
///
/// Fixed text, with the reserved keys the REQUEST used as its one variable part — which is
/// what tells a caller which of its own variables to drop. Listed in the order
/// [`RESERVED_ENVIRONMENT_KEYS`] states, so one request is refused one way.
#[must_use]
pub fn reserved_environment_keys_message(reserved: &[&str]) -> String {
    format!(
        "Lambda was unable to configure your environment variables because the environment \
         variables you have provided contains reserved keys that are currently not supported \
         for modification. Reserved keys used in this request: {}",
        reserved.join(", ")
    )
}

/// The environment variables an `Environment` member asks a function to hold, ordered so
/// a read reports them deterministically.
///
/// `Err` carries the message of the refusal, in the words AWS gives each: the set names a
/// key Lambda reserves for the runtime, or it is larger than the documented quota. Both
/// state what Lambda could not do and then the part of the REQUEST that stopped it — the
/// reserved keys the request used, and the whole set as it was measured — so a caller can
/// see which of its own variables to change. Every operation that accepts an `Environment`
/// applies the same two rules, so the check lives here rather than once per operation.
pub fn environment_variables(
    environment: Option<&Environment>,
) -> Result<BTreeMap<String, String>, String> {
    let variables: BTreeMap<String, String> = environment
        .and_then(|environment| environment.variables.clone())
        .map(|variables| variables.into_iter().collect())
        .unwrap_or_default();
    // The reserved keys are checked FIRST: a request naming one is refused for the key
    // whatever the set's size, because the key is what the caller has to drop.
    let reserved: Vec<&str> = RESERVED_ENVIRONMENT_KEYS
        .iter()
        .copied()
        .filter(|key| variables.contains_key(*key))
        .collect();
    if !reserved.is_empty() {
        return Err(reserved_environment_keys_message(&reserved));
    }
    let measured = measured_environment(&variables);
    if measured.len() > MAX_ENVIRONMENT_BYTES {
        return Err(environment_too_large_message(&measured));
    }
    Ok(variables)
}

/// A `FunctionName` the wire accepted, split into the parts that identify a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionReference {
    /// The bare function name.
    pub name: String,
    /// The qualifier the name carried, when it carried one.
    pub qualifier: Option<String>,
    /// Whether the reference was given as a full or partial ARN rather than a name.
    pub was_arn: bool,
    /// The REGION the reference named, when it was a full ARN — the only documented form
    /// that carries one. A bare name, the `function:` prefix form and the partial ARN name
    /// no region, so a request that uses one addresses the endpoint's own.
    pub region: Option<String>,
}

/// Split a `FunctionName` into its name and its qualifier.
///
/// The model documents the four forms a `FunctionName` may take — "Function name –
/// my-function[:qualifier]", "Function ARN –
/// arn:aws:lambda:us-west-2:123456789012:function:my-function[:qualifier]", "Partial
/// ARN – 123456789012:function:my-function[:qualifier]" — and its pattern also admits
/// the bare `function:my-function` prefix. All of them name the same record, so they
/// all resolve here to the same [`FunctionReference`].
///
/// `None` when the value is not any of those forms.
///
/// The value is UNTRUSTED input, so it is read without indexing: the full-ARN form goes
/// through the one shared parser (`service_common::arn`, which walks the six fields with
/// an iterator), and every other form is peeled with `strip_prefix` / `split_once`. No
/// length check can therefore go stale into an out-of-bounds index, and no input can make
/// this function panic.
#[must_use]
pub fn parse_function_reference(value: &str) -> Option<FunctionReference> {
    // Full ARN: arn:<partition>:lambda:<region>:<account>:function:<name>[:<qualifier>].
    // The resource type is required, so an ARN of another Lambda resource (a layer, a
    // code-signing config) is not read as a function named after its resource type.
    if value.starts_with("arn:") {
        let arn = service_common::arn::parse_of(value, ARN_NAMESPACE)?;
        let region = arn.region.to_string();
        let after_type = arn.resource.strip_prefix(FUNCTION_RESOURCE_TYPE_PREFIX)?;
        return name_and_qualifier(after_type, true, Some(region));
    }

    // Partial ARN: <account>:function:<name>[:<qualifier>]. The `function` literal is
    // required for the leading 12-digit segment to be read as an account id, because a
    // function name may itself be twelve digits: `123456789012:2` names version 2 of
    // the function `123456789012`, not the function `2`.
    if let Some((account, after_account)) = value.split_once(':') {
        if account.len() == 12 && account.bytes().all(|byte| byte.is_ascii_digit()) {
            if let Some(after_type) = after_account.strip_prefix(FUNCTION_RESOURCE_TYPE_PREFIX) {
                return name_and_qualifier(after_type, true, None);
            }
        }
    }

    // The pattern's bare `function:` prefix carries the resource type before the name.
    // It is not an ARN, so it does not lift the documented 64-character limit on a name
    // given without one.
    match value.strip_prefix(FUNCTION_RESOURCE_TYPE_PREFIX) {
        Some(after_type) => name_and_qualifier(after_type, false, None),
        None => name_and_qualifier(value, false, None),
    }
}

/// The resource-type prefix every form that carries one writes before the name.
const FUNCTION_RESOURCE_TYPE_PREFIX: &str = "function:";

/// `<name>[:<qualifier>]`, once whatever preceded the name has been peeled off.
///
/// `None` for a value that names no function: an empty name, an empty qualifier, or a
/// third `:`-separated field, none of which any documented form writes.
fn name_and_qualifier(
    value: &str,
    was_arn: bool,
    region: Option<String>,
) -> Option<FunctionReference> {
    let (name, qualifier) = match value.split_once(':') {
        None => (value, None),
        Some((name, qualifier)) if !qualifier.is_empty() && !qualifier.contains(':') => {
            (name, Some(qualifier))
        }
        Some(_) => return None,
    };
    (!name.is_empty()).then(|| FunctionReference {
        name: name.to_string(),
        qualifier: qualifier.map(str::to_string),
        was_arn,
        region,
    })
}

/// The refusal a `DurableExecutionArn` that names no execution this emulator holds gets:
/// the canonical sentence every Lambda resource-shaped refusal follows, naming the
/// resource the request asked about so the caller recovers the ARN it sent by stripping
/// the fixed prefix (compare the `Function not found: <arn>` [`resolve_function`] builds).
#[must_use]
pub fn durable_execution_not_found_message(arn: &str) -> String {
    format!("Durable execution not found: {arn}")
}

/// Resolve a `DurableExecutionArn` to the function record that holds the execution and
/// the name the execution is keyed by.
///
/// The ARN is the one form the model's `DurableExecutionArn` pattern admits —
/// `<function-arn>:<version>/durable-execution/<execution-name>/<execution-id>` — so it
/// is read as the function ARN it carries (resolved exactly the way every other operation
/// resolves a `FunctionName`, which is also what brings the record up to whatever the
/// clock made due) plus the execution's name and id. The id is CHECKED against the stored
/// execution, because an ARN that names the right name under a different id names an
/// execution this emulator never started.
///
/// The value is UNTRUSTED request text: it is only ever split and compared, so no input
/// can make this panic.
pub fn resolve_durable_execution<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    execution_arn: &str,
    region: &str,
    now: SystemTime,
) -> Result<(&'a mut FunctionRecord, String), ResolveFailure> {
    let not_found =
        || ResolveFailure::NotFound(durable_execution_not_found_message(execution_arn));
    let infix = format!("/{DURABLE_EXECUTION_ARN_INFIX}/");
    let Some((function_reference, execution)) = execution_arn.split_once(infix.as_str()) else {
        return Err(not_found());
    };
    let Some((name, execution_id)) = execution.split_once('/') else {
        return Err(not_found());
    };
    let (record, version) = resolve_function(store, function_reference, None, region, now)
        // A function this emulator does not hold holds no execution either, and the
        // refusal names the execution the request asked about rather than the function it
        // was derived from.
        .map_err(|_| not_found())?;
    let held = record
        .durable_executions
        .get(name)
        .filter(|held| held.execution_id == execution_id && held.version == version);
    match held {
        Some(_) => Ok((record, name.to_string())),
        None => Err(not_found()),
    }
}

/// The ARN one INVOCATION addresses: the function the `FunctionName`/`Qualifier` pair names,
/// at the version it resolves.
///
/// `Qualifier` "specif[ies] a version or alias to invoke a published version of the
/// function", and an invocation naming none runs the unpublished version — which is what an
/// invocation's answer reports as `ExecutedVersion` — so the qualifier is the one the name
/// carries, else the `Qualifier` parameter's, else [`VERSION_LATEST`]. ONE construction,
/// shared by every operation that INVOKES (`Invoke`, `InvokeWithResponseStream`), because
/// the resource they address is the same one: a refusal naming no single resolved ARN is
/// what a client comparing it against the ARN it invoked cannot match.
///
/// `None` is a `FunctionName` this operation resolves NO version for, so there is no
/// resolved ARN to name and the refusal is the resolution's own: a value that names no
/// function at all (which the resolution refuses as malformed), and an ARN of another
/// Region, which is refused on the Region before any version is resolved
/// ([`foreign_region_function_message`]).
#[must_use]
pub fn addressed_function_arn(
    region: &str,
    function_name: &str,
    qualifier: Option<&str>,
) -> Option<String> {
    let reference = parse_function_reference(function_name)?;
    addressed_arn_of(region, &reference, qualifier)
}

/// [`addressed_function_arn`] for a caller that has ALREADY parsed the reference, so no
/// caller carries a second reading of the same `FunctionName` — and none needs a fallback
/// for a parse that cannot fail twice.
#[must_use]
pub fn addressed_arn_of(
    region: &str,
    reference: &FunctionReference,
    qualifier: Option<&str>,
) -> Option<String> {
    if reference
        .region
        .as_deref()
        .is_some_and(|named| named != region)
    {
        return None;
    }
    let addressed = reference
        .qualifier
        .as_deref()
        .or(qualifier)
        .unwrap_or(VERSION_LATEST);
    Some(format!(
        "{}:{addressed}",
        function_arn(region, &reference.name)
    ))
}

/// The refusal a `FunctionName` that names a function of ANOTHER Region gets, or `None` when
/// the reference names this endpoint's own Region or names no Region at all.
///
/// A Lambda function is a REGIONAL resource: it lives in the Region it was created in and is
/// reachable only through that Region's endpoint. The full-ARN form is the only documented
/// `FunctionName` that carries a Region, so it is the only one this can refuse; a bare name,
/// the `function:` prefix form and the partial ARN address the endpoint's own Region.
///
/// The refusal states the two Regions that DISAGREE — the one the identifier named and the
/// one this endpoint serves — rather than naming the resource: the function it addressed may
/// well exist, so what is wrong is that it is not reachable from here.
///
/// [`resolve_function`] applies this to every operation that resolves a `FunctionName`
/// through it; an operation that keys the store by name itself applies it where it parses
/// the reference.
#[must_use]
pub fn foreign_region_function_message(
    reference: &FunctionReference,
    region: &str,
) -> Option<String> {
    reference
        .region
        .as_deref()
        .filter(|named| *named != region)
        .map(|named| {
            format!("Functions from '{named}' are not reachable in this region ('{region}')")
        })
}

/// The Region refusal a `FunctionName` gets, for an operation that answers its OWN sentence
/// for a resource it could not find.
///
/// A Region this endpoint does not serve is a fact about the REQUEST, settled before
/// anything is looked up, so it is not one of the things such an operation's not-found
/// sentence stands for — a resource that is absent here, or that holds no document of the
/// kind asked for. The two are told apart by reading the identifier, which is what this
/// does; `None` is an identifier naming no Region or naming this endpoint's own, and then
/// the refusal is the operation's own to word
/// ([`foreign_region_function_message`] states the refusal itself).
#[must_use]
pub fn unreachable_region_message(function_name: &str, region: &str) -> Option<String> {
    foreign_region_function_message(&parse_function_reference(function_name)?, region)
}

/// The refusal a request whose `FunctionName` qualifies one version and whose `Qualifier`
/// names another gets.
///
/// Fixed text: both halves came from the caller, so the sentence states only that the
/// qualifier DERIVED from the name is not the one the request specified.
pub const DERIVED_QUALIFIER_MISMATCH_MESSAGE: &str =
    "The derived qualifier from the function name does not match the specified qualifier.";

/// Why a `FunctionName`/`Qualifier` pair names no record.
#[derive(Debug, Clone)]
pub enum ResolveFailure {
    /// The pair is not a resource this service can name: an unparseable
    /// `FunctionName`, or a `FunctionName` and a `Qualifier` naming two different
    /// versions. The caller has to change the request.
    Malformed(String),
    /// The pair is well formed but names no function, or no such version of one.
    NotFound(String),
}

/// Resolve a `FunctionName` (in any of its documented forms, optionally qualified) and
/// an optional `Qualifier` query parameter to one stored record and one version label.
///
/// The pair is read as a REQUEST first and as a resource second: a `FunctionName` in none
/// of the documented forms is a parameter violation stated before anything is looked up,
/// as is a `Qualifier` outside the pattern its member declares — which each operation
/// declaring that member states for itself, because the expression a clause quotes is the
/// member's ([`admitted_qualifier`]).
///
/// The record is brought up to whatever the clock already made due before it is
/// handed back, so every caller reports the same lifecycle state at the same instant.
pub fn resolve_function<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    function_name: &str,
    qualifier: Option<&str>,
    region: &str,
    now: SystemTime,
) -> Result<(&'a mut FunctionRecord, String), ResolveFailure> {
    let reference = parse_function_reference(function_name).ok_or_else(|| {
        ResolveFailure::Malformed(format!(
            "1 validation error detected: Value '{function_name}' at 'functionName' failed to \
             satisfy constraint: Member must satisfy one of the documented name formats (name, \
             name:qualifier, partial ARN or function ARN)"
        ))
    })?;

    // Both the name's own qualifier and the `Qualifier` parameter name a version, so a
    // request carrying two different ones names no single resource. The refusal states the
    // requirement and nothing of the request: the two values are the caller's own, and the
    // sentence says which of them disagree.
    let version = match (reference.qualifier.as_deref(), qualifier) {
        (Some(from_name), Some(from_query)) if from_name != from_query => {
            // The sentence is AWS's fixed text for this condition and carries NOTHING of
            // the request: the caller sent both halves, so naming them back says no more
            // than that the two disagree.
            return Err(ResolveFailure::Malformed(
                DERIVED_QUALIFIER_MISMATCH_MESSAGE.to_string(),
            ))
        }
        (Some(from_name), _) => from_name.to_string(),
        (None, Some(from_query)) => from_query.to_string(),
        (None, None) => VERSION_LATEST.to_string(),
    };

    // A Lambda function is a REGIONAL resource: it lives in the Region it was created in
    // and is reachable only through that Region's endpoint. So a reference that NAMES a
    // Region — the full-ARN form is the only documented one that does — names a function of
    // this endpoint only when the two Regions are the same one, and names nothing here
    // otherwise, exactly as a name no function of this Region is held under does
    // ([`foreign_region_function_message`]).
    if let Some(message) = foreign_region_function_message(&reference, region) {
        return Err(ResolveFailure::NotFound(message));
    }

    let Some(record) = store.functions.get_mut(&reference.name) else {
        return Err(ResolveFailure::NotFound(format!(
            "Function not found: {}",
            function_arn(region, &reference.name)
        )));
    };
    record.materialize(now);
    if record.config_of(&version).is_none() {
        let arn = record.versioned_arn(&version);
        return Err(ResolveFailure::NotFound(format!("Function not found: {arn}")));
    }
    Ok((record, version))
}

/// Resolve a `PolicyResourceArn` — the `{ResourceArn}` label the resource-policy
/// operations bind — to the record that holds that resource's policy, and the qualifier
/// the policy is keyed by.
///
/// The member's own pattern admits two kinds of Lambda resource
/// (`smithy_query(query="operation", operation="PutResourcePolicy")`): a function,
/// optionally qualified by a version or an alias, and a runtime (`runtime/<id>`). A
/// function ARN is resolved exactly as every other operation on this service resolves a
/// `FunctionName`, so one resource has one policy however it is addressed; a runtime is
/// a resource this store holds no records of, so an ARN naming one names no resource
/// here.
pub fn resolve_policy_resource<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    resource_arn: &str,
    region: &str,
    now: SystemTime,
) -> Result<(&'a mut FunctionRecord, String), ResolveFailure> {
    if parse_function_reference(resource_arn).is_none_or(|reference| !reference.was_arn) {
        return Err(ResolveFailure::NotFound(
            NO_RESOURCE_POLICY_MESSAGE.to_string(),
        ));
    }
    resolve_function(store, resource_arn, None, region, now)
}

/// One layer version a `LayerName`/`VersionNumber` pair resolved to: the ARN that pair
/// names, and the stored version itself.
///
/// The ARN comes along because it is the `Resource` every policy statement on this
/// version is granted on, and the resource a refusal names; it is built from the stored
/// layer record rather than from the request.
#[derive(Debug)]
pub struct ResolvedLayerVersion<'a> {
    /// The ARN of the resolved version, as `LayerVersionArn` spells it.
    pub arn: String,
    /// The resolved version.
    pub version: &'a mut LayerVersionRecord,
}

/// What [`ResolvedLayerVersion`] is to a write: the same pair resolved for a REPORT, so
/// a reporting operation cannot reach a mutable borrow of the store at all.
#[derive(Debug)]
pub struct ResolvedLayerVersionRef<'a> {
    /// The ARN of the resolved version, as `LayerVersionArn` spells it.
    pub arn: String,
    /// The resolved version.
    pub version: &'a LayerVersionRecord,
}

/// The bare name a `LayerName` names for a request made in `region`, or the refusal the
/// value gets ([`layer_name_in`]).
///
/// "The name or Amazon Resource Name (ARN) of the layer": both forms name the same
/// record, so both resolvers below start here.
fn layer_name_of(layer_name: &str, region: &str) -> Result<String, ResolveFailure> {
    layer_name_in(layer_name, region).map_err(ResolveFailure::Malformed)
}

/// The refusal a `LayerName`/`VersionNumber` pair naming no stored version gets on the
/// POLICY operations that share these resolvers.
///
/// Here the layer version is the operation's OWN resource, so the refusal identifies it:
/// that is what lets a caller tell "no such version" from "that version holds no policy",
/// which is the state every layer-sharing bootstrap starts from. The READ of the version
/// itself (`GetLayerVersion`, `GetLayerVersionByArn`) answers the canonical
/// [`RESOURCE_NOT_FOUND_MESSAGE`] instead, as AWS does for it.
///
/// What identifies the version is its FULL `LayerVersionArn` — the ARN of the layer with
/// the version number appended, which is the identifier every answer about a layer version
/// carries ([`LayerRecord::version_arn`]) — and the sentence is a sentence, so it ends in a
/// stop. The ARN is built for the Region the request was made in, because that is the
/// Region whose layers it addressed, and a name this account never published has no stored
/// ARN to read one off.
fn no_such_layer_version(region: &str, name: &str, version_number: i64) -> ResolveFailure {
    ResolveFailure::NotFound(format!(
        "Layer version {arn}:{version_number} does not exist.",
        arn = layer_arn(region, name)
    ))
}

/// Resolve a `LayerName` (a bare name or a layer ARN) and a `VersionNumber` to the one
/// stored layer version they name, for an operation that CHANGES it.
///
/// Shared by every operation on a layer version's resource-based policy, so all of them
/// read the same record and refuse an unknown layer or version the same way.
pub fn resolve_layer_version<'a>(
    store: &'a mut ComAmazonawsLambdaAwsgirapiserviceState,
    layer_name: &str,
    version_number: i64,
    region: &str,
) -> Result<ResolvedLayerVersion<'a>, ResolveFailure> {
    // A layer is a REGIONAL resource, so the record is the one published in the Region
    // this request addressed; a layer of any other Region is not held here at all, and a
    // `LayerName` that NAMES another Region is refused rather than read as this one's.
    let name = layer_name_of(layer_name, region)?;
    let record = store
        .layer_mut(region, &name)
        .ok_or_else(|| no_such_layer_version(region, &name, version_number))?;
    let arn = record.version_arn(version_number);
    let version = record
        .versions
        .get_mut(&version_number)
        .ok_or_else(|| no_such_layer_version(region, &name, version_number))?;
    Ok(ResolvedLayerVersion { arn, version })
}

/// [`resolve_layer_version`] for an operation that only REPORTS on the version, which
/// therefore takes the store by shared reference and can change nothing.
pub fn resolve_layer_version_ref<'a>(
    store: &'a ComAmazonawsLambdaAwsgirapiserviceState,
    layer_name: &str,
    version_number: i64,
    region: &str,
) -> Result<ResolvedLayerVersionRef<'a>, ResolveFailure> {
    // Region-scoped for the same reason as [`resolve_layer_version`].
    let name = layer_name_of(layer_name, region)?;
    let record = store
        .layer(region, &name)
        .ok_or_else(|| no_such_layer_version(region, &name, version_number))?;
    let arn = record.version_arn(version_number);
    let version = record
        .versions
        .get(&version_number)
        .ok_or_else(|| no_such_layer_version(region, &name, version_number))?;
    Ok(ResolvedLayerVersionRef { arn, version })
}

/// The REGION a layer belongs to: how this store is keyed by it, which layers a request
/// can reach, and the rules a function's `Layers` list has to satisfy; see the module's
/// own header.
pub mod layer_store;
pub use layer_store::*;

#[cfg(test)]
mod tests;

/// How a request NAMES a function's deployment package, and what each package type admits;
/// see the module's own header.
mod package_code;
pub use package_code::*;

/// Which RESOURCE a tagging request addresses, and what a request naming one this store
/// holds no record of is answered — the whole reading behind the
/// `/2017-03-31/tags/{Resource}` operations; see the module's own header.
mod resource_tags;
pub use resource_tags::*;

/// The constraints a tag KEY carries, which no request boundary applies, and the refusal a
/// request naming one outside them gets; see the module's own header.
mod tag_key_shape;
pub use tag_key_shape::{is_shaped_tag_key, tag_key_shape_refusal};

/// Which LOG FORMAT a function's level filters may be configured on; see the module's own
/// header.
mod log_level_format;
pub use log_level_format::{log_level_format_refusal, LOG_LEVELS_NEED_JSON_MESSAGE};

/// WHERE a layer version's archive and a function's deployment package are downloaded
/// from: the presigned URL a read announces for one, and the object a request to that URL
/// names; see the module's own header.
pub mod download_location;
pub use download_location::*;
