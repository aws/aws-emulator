// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appflow-sandstoneconfigurationservicelambda service.
//!
//! The AppFlow control plane this emulator serves is a store of FLOWS, each one
//! carrying the definition `CreateFlow` persisted plus the run history `StartFlow`
//! appends. Every operation computes its answer from this store: nothing here is
//! derived from the request that is reading it.
//!
//! The modelled shapes (`TriggerConfig`, `SourceFlowConfig`, `Task`, …) are stored as
//! they arrived rather than re-modelled, so `DescribeFlow` can return exactly what
//! `CreateFlow` was given — with the configuration containers AppFlow itself materialises
//! filled in on the way in (see [`materialize_definition_containers`]), because they are
//! part of the definition the service keeps.
//!
//! Beside the account's own resources it also holds the CATALOG of connectors AppFlow
//! vends — a property of the service rather than of the account, which is why it is a
//! table here and not a map a request can write (see [`VENDED_CONNECTORS`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    ConnectorConfiguration, ConnectorMetadata, DestinationFlowConfig, ErrorInfo, ExecutionDetails,
    ExecutionResult, FlowDefinition, MetadataCatalogConfig, SourceFlowConfig, Task, TriggerConfig,
};

/// WHO a write of a flow is attributed to: the principal the request's credential
/// speaks for.
mod caller_identity;
/// What a flow RUN does: the transfer `StartFlow` performs between the source and the
/// destination a flow's definition names.
mod flow_run;
/// The sentences this service refuses a request with.
mod refusals;

pub use caller_identity::*;
pub use flow_run::*;
pub use refusals::*;

/// The region an ARN carries when the request is not SigV4-signed (an anonymous
/// request names no region). Generated emulators default to `us-east-1`; see
/// `emulator_server::emulator_arn`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `triggerType` of a flow AppFlow runs only when asked
/// (`smithy_query(query="shape", shape="TriggerType")`).
pub const TRIGGER_TYPE_ON_DEMAND: &str = "OnDemand";

/// The `triggerType` of a flow AppFlow runs on a schedule
/// (`smithy_query(query="shape", shape="TriggerType")`).
pub const TRIGGER_TYPE_SCHEDULED: &str = "Scheduled";

/// The `triggerType` of a flow an event in the source application runs
/// (`smithy_query(query="shape", shape="TriggerType")`).
pub const TRIGGER_TYPE_EVENT: &str = "Event";

/// How long a run stays in a non-terminal status: how long after `StartFlow` it stops
/// being `InProgress`, and how long after `CancelFlowExecutions` it stops being
/// `CancelStarted`.
///
/// CHOSEN, not documented: AppFlow publishes no duration for a flow run or for the
/// cancellation of one, so this is the emulator's own span — long enough that a caller
/// which polls straight after either call still observes the non-terminal run (and the
/// state errors that go with it), short enough that a test need not wait. The instant is
/// stored on the run and materialised by whichever call reads it next; nothing sleeps.
pub const EXECUTION_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// Why a flow run cannot succeed in the emulator.
///
/// A flow run is a data transfer between the source and destination connector
/// endpoints — an outbound connection the emulator does not make — so no transfer
/// ever happens and reporting a successful one would be a fabricated result. The run
/// is therefore recorded, and then ends in the service's own modelled failure state
/// (`ExecutionStatus::Error`) carrying this reason.
///
/// The wording stays inside both message patterns it has to satisfy:
/// `ErrorInfo$executionMessage` (`^[\s\w/!@#+=.-]*$`) and
/// `ExecutionDetails$mostRecentExecutionMessage` (`^[\w!@#\-.?,\s]*$`) — so no comma,
/// colon, slash or apostrophe.
pub const EXECUTION_NOT_PERFORMED_MESSAGE: &str = "This emulator does not connect to source or destination connector endpoints so the flow run was not performed.";

/// The listing a `ListFlows` cursor belongs to.
pub const FLOW_CURSOR: &str = "flows";

/// The listing a `DescribeFlowExecutionRecords` cursor belongs to.
pub const EXECUTION_CURSOR: &str = "flow-executions";

/// The most flows one account may hold, which `CreateFlow` refuses to exceed with the
/// modelled `ServiceQuotaExceededException` ("The request would cause a service quota
/// (such as the number of flows) to be exceeded").
///
/// CHOSEN, not documented: AppFlow does publish a per-account flow quota, but it is
/// adjustable and not stated in the model, so this is the value this emulator enforces
/// rather than a cited one. It also bounds
/// [`ComAmazonawsAppflowSandstoneconfigurationservicelambdaState::flows`], so a
/// long-lived process cannot grow without limit.
pub const MAX_FLOWS_PER_ACCOUNT: usize = 1000;

/// How many runs one flow keeps.
///
/// CHOSEN: AppFlow publishes no retention for flow-run records, so this is the
/// emulator's own window. It bounds [`Flow::executions`] — and with it every per-flow
/// walk, so a flow that has been started many times still costs a bounded amount of
/// work and memory. The OLDEST record is dropped first, so the runs a caller is most
/// likely to read are the ones kept.
pub const MAX_EXECUTIONS_PER_FLOW: usize = 100;

/// `true` when a `Scheduled` trigger carries no schedule.
///
/// A schedule is what makes a scheduled flow run, so AppFlow cannot store such a
/// definition without one. Shared by every operation that stores a definition
/// (`CreateFlow`, `UpdateFlow`), so the two cannot enforce it differently.
pub fn scheduled_trigger_missing_schedule(trigger_config: &TriggerConfig) -> bool {
    trigger_config.trigger_type == TRIGGER_TYPE_SCHEDULED
        && trigger_config
            .trigger_properties
            .as_ref()
            .and_then(|properties| properties.scheduled.as_ref())
            .is_none()
}

/// The status a flow is STORED with, decided by the trigger that runs it.
///
/// A flow AppFlow runs only when a caller asks for it is finished the moment it is
/// stored, so it is created ACTIVE and can be run straight away. A flow that runs
/// itself — on a schedule, or on an event in the source application — is created as a
/// DRAFT, because its trigger has not been armed yet; activating it is what `StartFlow`
/// does for such a flow (see [`Flow::activate_trigger`]).
///
/// Shared by every operation that stores a new definition, so no two of them can decide
/// a flow's initial status differently. The two value sets are the model's
/// (`smithy_query(query="shape", shape="FlowStatus,TriggerType")`); which status goes
/// with which trigger type is recorded behaviour.
pub fn initial_flow_status(trigger_config: &TriggerConfig) -> FlowStatus {
    if trigger_config.trigger_type == TRIGGER_TYPE_ON_DEMAND {
        FlowStatus::Active
    } else {
        FlowStatus::Draft
    }
}

/// Fill in the configuration containers AppFlow MATERIALISES on the flow it stores.
///
/// A flow's stored definition is not the sparse request body it arrived in: AppFlow
/// fills in the containers whose contents it defaults — the trigger's properties, each
/// task's properties, the prefix configuration of an S3 output format, and the metadata
/// catalog configuration — so every flow carries them, empty where nothing inside was
/// configured, and a read of the flow answers with the container rather than omitting
/// it. The members INSIDE a container are left exactly as they arrived: nothing here
/// invents a value.
///
/// Shared by every operation that stores a definition (`CreateFlow`, `UpdateFlow`), so
/// the two cannot store two different shapes of the same flow — and applied before a
/// definition is compared with a stored one, so a retry carrying the sparse body still
/// compares equal to what was stored for it.
pub fn materialize_definition_containers(
    trigger_config: &mut TriggerConfig,
    destination_flow_config_list: &mut [DestinationFlowConfig],
    tasks: &mut [Task],
    metadata_catalog_config: &mut Option<MetadataCatalogConfig>,
) {
    trigger_config.trigger_properties.get_or_insert_default();
    for destination in destination_flow_config_list {
        // Only the S3 destination has an output format whose prefix configuration is
        // optional; Upsolver's `UpsolverS3OutputFormatConfig$prefixConfig` is required,
        // so it is already present on every stored flow.
        if let Some(output_format) = destination
            .destination_connector_properties
            .s3
            .as_mut()
            .and_then(|s3| s3.s3_output_format_config.as_mut())
        {
            output_format.prefix_config.get_or_insert_default();
        }
    }
    for task in tasks {
        task.task_properties.get_or_insert_default();
    }
    metadata_catalog_config.get_or_insert_default();
}

/// The KMS key AppFlow manages for an account, as the key ARN a flow reports.
///
/// "This is required if you do not want to use the Amazon AppFlow-managed KMS key. If
/// you don't provide anything here, Amazon AppFlow uses the Amazon AppFlow-managed KMS
/// key" (`smithy_query(query="doc", shape="CreateFlowRequest", member="kmsArn")`) — so
/// every flow has a key, and a read of a flow reports the key that encrypts it rather
/// than only a key the caller chose to name.
///
/// The key ID is the one KMS minted for the key, so it is opaque: this emulator mints it
/// ONCE for the account (see
/// [`ComAmazonawsAppflowSandstoneconfigurationservicelambdaState::managed_kms_key_arn`])
/// and every flow that names no key of its own reports that same key, which is what
/// makes it the account's managed key rather than a value made up per flow. The ARN
/// satisfies `KMSArn`'s `^arn:aws:kms:.*:[0-9]+:`
/// (`smithy_query(query="shape", shape="KMSArn")`).
pub fn kms_key_arn(region: &str, key_id: &str) -> String {
    ::emulator_server::emulator_arn("kms", region, &format!("key/{key_id}"))
}

/// A flow's `flowStatus` (`smithy_query(query="shape", shape="FlowStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStatus {
    Active,
    Deleted,
    Deprecated,
    Draft,
    Errored,
    Suspended,
}

impl FlowStatus {
    /// The wire value, exactly as the `FlowStatus` enum declares it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Deleted => "Deleted",
            Self::Deprecated => "Deprecated",
            Self::Draft => "Draft",
            Self::Errored => "Errored",
            Self::Suspended => "Suspended",
        }
    }
}

/// A run's `executionStatus` (`smithy_query(query="shape", shape="ExecutionStatus")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStatus {
    CancelStarted,
    Canceled,
    Error,
    InProgress,
    Successful,
}

impl ExecutionStatus {
    /// The wire value, exactly as the `ExecutionStatus` enum declares it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CancelStarted => "CancelStarted",
            Self::Canceled => "Canceled",
            Self::Error => "Error",
            Self::InProgress => "InProgress",
            Self::Successful => "Successful",
        }
    }
}

/// What the clock owes a run when its span elapses: the status it ends in and the
/// result it reports.
///
/// Both are decided when the run is STARTED, by the transfer that run performed then
/// (see [`perform_run`]) — the work is what the run did, and only the TRANSITION to its
/// terminal status is owed by the clock. So a read that materialises the transition
/// reports the outcome of work already done rather than performing any.
#[derive(Debug, Clone)]
pub struct ExecutionOutcome {
    pub status: ExecutionStatus,
    pub result: ExecutionResult,
    /// When the run began and finished reading the source, for a run that read one.
    pub data_pull_start_time: Option<SystemTime>,
    pub data_pull_end_time: Option<SystemTime>,
}

/// One recorded run of a flow, as `StartFlow` appended it.
#[derive(Debug, Clone)]
pub struct FlowExecution {
    pub execution_id: String,
    /// The `@idempotencyToken` of the `StartFlow` that recorded this run, when it
    /// carried one. Held HERE rather than in a store-wide map, so the token is
    /// reclaimed with the run it identifies and cannot accumulate without bound.
    pub client_token: Option<String>,
    pub execution_status: ExecutionStatus,
    pub execution_result: Option<ExecutionResult>,
    pub started_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// When the clock owes this run its terminal status. `None` once settled.
    pub settles_at: Option<SystemTime>,
    /// The outcome of the transfer this run PERFORMED when it was started, which the
    /// clock owes it at [`FlowExecution::settles_at`]. `None` for a run whose work this
    /// emulator could not perform at all, which ends in the service's modelled failure
    /// state instead.
    pub outcome: Option<ExecutionOutcome>,
    pub data_pull_start_time: Option<SystemTime>,
    pub data_pull_end_time: Option<SystemTime>,
}

impl FlowExecution {
    /// The wire `ExecutionRecord` for this run.
    ///
    /// Every member is read off the stored run: the status it reached, the counters the
    /// transfer it performed came to, and the two instants it carries — reported both
    /// under the names the model declares (`startedAt`, `lastUpdatedAt`) and under the
    /// names AppFlow reports beside them (`executionStartTime`, `lastUpdatedTime`), so
    /// the two spellings of one instant can never disagree.
    pub fn to_execution_record(&self) -> crate::ops::types::ExecutionRecord {
        let execution_message = self.execution_message();
        crate::ops::types::ExecutionRecord {
            execution_id: Some(self.execution_id.clone()),
            execution_status: Some(self.execution_status.as_str().to_string()),
            execution_result: self.execution_result.clone(),
            started_at: Some(to_date_time(self.started_at)),
            last_updated_at: Some(to_date_time(self.last_updated_at)),
            data_pull_start_time: self.data_pull_start_time.map(to_date_time),
            data_pull_end_time: self.data_pull_end_time.map(to_date_time),
            // No catalog registration happens: nothing was transferred to catalog.
            metadata_catalog_details: None,
            execution_message,
            // A run that has not finished has no accounting yet; one that has reports it.
            execution_metadata: self.execution_result.as_ref().map(execution_metadata),
            execution_start_time: Some(to_date_time(self.started_at)),
            last_updated_time: Some(to_date_time(self.last_updated_at)),
        }
    }

    /// What this run has to say for itself in one sentence, as `executionMessage`.
    ///
    /// A run that SUCCEEDED is reported with AppFlow's own fixed text for that outcome
    /// ([`RUN_SUCCEEDED_MESSAGE`]) — it has no variable part. A run that FAILED is
    /// reported with the reason its `errorInfo` already carries, so the sentence and the
    /// error information can never disagree. A run that has not reached a terminal status,
    /// and one that was cancelled rather than run, has nothing to report: the member is
    /// then absent rather than filled with a sentence nothing backs.
    fn execution_message(&self) -> Option<String> {
        match self.execution_status {
            ExecutionStatus::Successful => Some(RUN_SUCCEEDED_MESSAGE.to_string()),
            ExecutionStatus::Error => self.failure_reason(),
            ExecutionStatus::Canceled
            | ExecutionStatus::CancelStarted
            | ExecutionStatus::InProgress => None,
        }
    }

    /// Why this run did not do what it was asked to, as its own result states it, or
    /// `None` for a run that has nothing to report.
    fn failure_reason(&self) -> Option<String> {
        self.execution_result
            .as_ref()?
            .error_info
            .as_ref()?
            .execution_message
            .clone()
    }

    /// `true` while this run has not reached a terminal status: it is still in
    /// progress, or its cancellation has started and has not completed yet.
    ///
    /// It is also exactly the window in which the run can be cancelled: a run may be
    /// cancelled "only when the run is in progress", never one that "has already
    /// completed or failed" nor one "that's scheduled to occur but hasn't started yet"
    /// (`smithy_query(query="doc", shape="CancelFlowExecutions")`), and a run whose
    /// cancellation is already under way is that same request repeated rather than an
    /// ineligible run.
    ///
    /// Call [`Flow::settle_due_executions`] first, or a run the clock has already
    /// finished still reads as active.
    pub fn is_active(&self) -> bool {
        matches!(
            self.execution_status,
            ExecutionStatus::InProgress | ExecutionStatus::CancelStarted
        )
    }

    /// Begin cancelling this run, answering whether it was eligible at all.
    ///
    /// "When you send your request, the status for each run becomes CancelStarted. When
    /// the cancellation completes, the status becomes Canceled."
    /// (`smithy_query(query="doc", shape="CancelFlowExecutions")`) — so this call only
    /// starts the cancellation and the clock owes the run its `Canceled` status, which
    /// [`Flow::settle_due_executions`] materialises. A run whose cancellation had
    /// already started keeps the instant the FIRST request set, so repeating the
    /// request cannot postpone it.
    pub fn begin_cancellation(&mut self, now: SystemTime) -> bool {
        if !self.is_active() {
            return false;
        }
        if self.execution_status == ExecutionStatus::InProgress {
            self.execution_status = ExecutionStatus::CancelStarted;
            self.last_updated_at = now;
            self.settles_at = Some(now + EXECUTION_SETTLES_AFTER);
        }
        true
    }
}

/// AppFlow's `executionMessage` for a run that succeeded: fixed text, with no variable
/// part — the run's own volumes are reported by `executionResult` and
/// `executionMetadata`, not by this sentence.
pub const RUN_SUCCEEDED_MESSAGE: &str = "Successfully ran the flow";

/// The `executionMetadata` of a run that has finished: that run's own accounting, taken
/// from the result the transfer it performed reached.
///
/// Nothing here is counted a second time: the volumes are the ones `executionResult`
/// reports, so the two blocks of one record cannot disagree. The failure counts are the
/// run's own put failures — a run of this emulator transfers every record it read or none
/// at all, so the records that failed are exactly the ones the destination did not take —
/// `numFilteredRecords` is zero because a flow's `tasks` project the FIELDS a record is
/// written with and drop no record, and `terminateFlow` is false because no run here asks
/// for its flow to be stopped.
fn execution_metadata(result: &ExecutionResult) -> crate::ops::types::ExecutionMetadata {
    let put_failures = result
        .error_info
        .as_ref()
        .and_then(|error_info| error_info.put_failures_count)
        .unwrap_or(0);
    crate::ops::types::ExecutionMetadata {
        bytes_processed: result.bytes_processed,
        bytes_written: result.bytes_written,
        max_page_size: result.max_page_size,
        num_failures: Some(put_failures),
        num_filtered_records: Some(0),
        num_parallel_processes: result.num_parallel_processes,
        num_put_failures: Some(put_failures),
        reason: result
            .error_info
            .as_ref()
            .and_then(|error_info| error_info.execution_message.clone()),
        records_processed: result.records_processed,
        terminate_flow: Some(false),
    }
}

/// The `executionResult` of a run that transferred nothing: every counter a true zero
/// rather than an invented volume, plus whatever that run has to say for itself.
///
/// Every counter the shape declares is REPORTED, zero included: a run that has finished
/// accounts for each dimension of what it did, so a client reading one of them can tell
/// "none" from "not reported". `numParallelProcesses` is zero because no process ran the
/// transfer, and `maxPageSize` because no page size bounded one.
fn nothing_transferred(error_info: Option<ErrorInfo>) -> ExecutionResult {
    ExecutionResult {
        error_info,
        bytes_processed: Some(0),
        bytes_written: Some(0),
        records_processed: Some(0),
        num_parallel_processes: Some(0),
        max_page_size: Some(0),
    }
}

/// A flow: the definition `CreateFlow` persisted, plus its run history.
#[derive(Debug, Clone)]
pub struct Flow {
    pub flow_arn: String,
    pub flow_name: String,
    /// The `@idempotencyToken` of the `CreateFlow` that stored this flow, when it
    /// carried one. Held HERE rather than in a store-wide map, so the token is
    /// reclaimed by `DeleteFlow` with the flow it identifies.
    pub client_token: Option<String>,
    /// The `@idempotencyToken` of the LAST `UpdateFlow` that re-defined this flow, when
    /// it carried one. Held beside the definition that token applied, so a retry of that
    /// same update is answered without re-applying it, and a DIFFERENT request reusing
    /// the token is refused. Separate from [`Flow::client_token`], which belongs to the
    /// create: the two operations are different calls and their tokens are their own.
    pub last_update_client_token: Option<String>,
    pub description: Option<String>,
    pub kms_arn: Option<String>,
    pub flow_status: FlowStatus,
    pub flow_status_message: Option<String>,
    pub trigger_config: TriggerConfig,
    pub source_flow_config: SourceFlowConfig,
    pub destination_flow_config_list: Vec<DestinationFlowConfig>,
    pub tasks: Vec<Task>,
    pub tags: BTreeMap<String, String>,
    pub metadata_catalog_config: Option<MetadataCatalogConfig>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub created_by: String,
    pub last_updated_by: String,
    pub schema_version: i64,
    /// Oldest first, as `StartFlow` appended them.
    pub executions: Vec<FlowExecution>,
}

impl Flow {
    /// `true` when the flow is run only on request, rather than on a schedule or an
    /// event.
    pub fn is_on_demand(&self) -> bool {
        self.trigger_config.trigger_type == TRIGGER_TYPE_ON_DEMAND
    }

    /// Arm the trigger of a flow that runs ITSELF, answering whether this call armed it.
    ///
    /// A schedule- or event-triggered flow is stored as a draft (see
    /// [`initial_flow_status`]); `StartFlow` on such a flow activates its trigger
    /// instead of running it now, which is why that call "[r]eturns the internal
    /// execution ID of an on-demand flow when the flow is started. For scheduled or
    /// event-triggered flows, this value is null"
    /// (`smithy_query(query="doc", shape="StartFlowResponse", member="executionId")`).
    /// An on-demand flow has no trigger to arm — it is already active — so nothing moves
    /// for one.
    ///
    /// A flow whose trigger was DEACTIVATED ([`Flow::deactivate_trigger`]) is armed again
    /// by the same call: activation is about the trigger's current state, so a suspended
    /// flow resumes rather than being refused.
    pub fn activate_trigger(&mut self) -> bool {
        if self.is_on_demand()
            || !matches!(self.flow_status, FlowStatus::Draft | FlowStatus::Suspended)
        {
            return false;
        }
        self.flow_status = FlowStatus::Active;
        true
    }

    /// Disarm the trigger of a flow that runs ITSELF, answering whether this call
    /// disarmed it.
    ///
    /// `StopFlow` "[d]eactivates the existing flow. For on-demand flows, this operation
    /// returns an unsupportedOperationException error message. For schedule and
    /// event-triggered flows, this operation deactivates the flow"
    /// (`smithy_query(query="doc", shape="StopFlow")`), and a deactivated flow is
    /// `Suspended` — the modelled status of a flow whose trigger is no longer armed
    /// (`smithy_query(query="shape", shape="FlowStatus")`). An on-demand flow has no
    /// trigger to disarm, and nor has one that is not active, so nothing moves for
    /// either.
    pub fn deactivate_trigger(&mut self) -> bool {
        if self.is_on_demand() || self.flow_status != FlowStatus::Active {
            return false;
        }
        self.flow_status = FlowStatus::Suspended;
        true
    }

    /// The DEFINITION this flow was created with, as one comparable value, so a retried
    /// create carrying the same `@idempotencyToken` can be told apart from a different
    /// request reusing that token. Computed from the stored members rather than held
    /// beside them, so the two cannot disagree.
    pub fn definition_descriptor(&self) -> Option<::serde_json::Value> {
        definition_descriptor(
            &self.flow_name,
            self.description.as_deref(),
            self.kms_arn.as_deref(),
            &self.trigger_config,
            &self.source_flow_config,
            &self.destination_flow_config_list,
            &self.tasks,
            &self.tags,
            self.metadata_catalog_config.as_ref(),
        )
    }

    /// The DATA SCHEMA this flow's definition declares, as one comparable value.
    ///
    /// It is drawn from the flow's `tasks`: the field mappings (a `Map` task's source
    /// and destination fields), the field data types (`SOURCE_DATA_TYPE` /
    /// `DESTINATION_DATA_TYPE` / `DATA_TYPE` task properties) and the partition keys (a
    /// `Partition` task's `ORDERED_PARTITION_KEYS_LIST` property) all live there — see
    /// `smithy_query(query="shape", shape="Task,TaskType,OperatorPropertiesKeys")`.
    /// Those are exactly the settings `schemaVersion` counts a change to
    /// (`smithy_query(query="doc", shape="DescribeFlowResponse", member="schemaVersion")`).
    pub fn schema_descriptor(&self) -> ::serde_json::Value {
        schema_descriptor(&self.tasks)
    }

    /// Give every run the terminal status the CLOCK already owed it.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `settles_at` whether or not anybody called, so materialising it on read reports
    /// the state the clock had already reached — it never moves a status because
    /// something was read.
    ///
    /// WHICH terminal status is owed follows from the status the run is leaving: a run
    /// whose cancellation had started completes as `Canceled`, and any other pending run
    /// ends with the outcome of the transfer it performed when it was started
    /// ([`FlowExecution::outcome`]) — or, for a run whose work this emulator could not
    /// perform at all, in the service's modelled failure state.
    pub fn settle_due_executions(&mut self, now: SystemTime) {
        for execution in &mut self.executions {
            let Some(due) = execution.settles_at else {
                continue;
            };
            if due > now {
                continue;
            }
            execution.settles_at = None;
            execution.last_updated_at = due;
            if execution.execution_status == ExecutionStatus::CancelStarted {
                // The cancellation this run was already undergoing has completed, so it
                // ends cancelled rather than failed, and nothing went wrong for an
                // `errorInfo` to report.
                execution.execution_status = ExecutionStatus::Canceled;
                execution.execution_result = Some(nothing_transferred(None));
                continue;
            }
            match execution.outcome.take() {
                Some(outcome) => {
                    execution.execution_status = outcome.status;
                    execution.execution_result = Some(outcome.result);
                    execution.data_pull_start_time = outcome.data_pull_start_time;
                    execution.data_pull_end_time = outcome.data_pull_end_time;
                }
                None => {
                    // No data was pulled, so there is no data-pull window to report: both
                    // instants stay `None` rather than describing a transfer that never
                    // ran.
                    execution.execution_status = ExecutionStatus::Error;
                    execution.execution_result = Some(nothing_transferred(Some(ErrorInfo {
                        put_failures_count: Some(0),
                        execution_message: Some(EXECUTION_NOT_PERFORMED_MESSAGE.to_string()),
                    })));
                }
            }
        }
    }

    /// Record a run, keeping at most [`MAX_EXECUTIONS_PER_FLOW`] of them.
    pub fn record_execution(&mut self, execution: FlowExecution) {
        self.executions.push(execution);
        if self.executions.len() > MAX_EXECUTIONS_PER_FLOW {
            let overflow = self.executions.len() - MAX_EXECUTIONS_PER_FLOW;
            self.executions.drain(..overflow);
        }
    }

    /// Attach to the run `execution_id` the outcome the transfer it performed reached, and
    /// hand it to the clock: from `now` the run is owed its terminal status, which a later
    /// read materialises.
    ///
    /// The outcome is attached AFTER the work, because the work does not run under this
    /// store's guard (see [`perform_run`]) — so a caller reading in between sees the run
    /// still in progress, which it is. `None` is a run whose work this emulator could not
    /// perform at all; such a run ends in the service's modelled failure state.
    ///
    /// A run whose cancellation has already started keeps the instant that cancellation
    /// set, so finishing the work cannot postpone it.
    pub fn attach_run_outcome(
        &mut self,
        execution_id: &str,
        outcome: Option<ExecutionOutcome>,
        now: SystemTime,
    ) {
        let Some(execution) = self
            .executions
            .iter_mut()
            .find(|execution| execution.execution_id == execution_id)
        else {
            return;
        };
        execution.outcome = outcome;
        if execution.execution_status == ExecutionStatus::InProgress {
            execution.settles_at = Some(now + EXECUTION_SETTLES_AFTER);
        }
    }
    /// The run a repeated `StartFlow` token already recorded, if any.
    pub fn execution_for_client_token(&self, client_token: &str) -> Option<&FlowExecution> {
        self.executions
            .iter()
            .find(|execution| execution.client_token.as_deref() == Some(client_token))
    }

    /// `true` while one of this flow's runs has not finished — see
    /// [`FlowExecution::is_active`].
    ///
    /// Call [`Flow::settle_due_executions`] first, or a run the clock has already
    /// finished still reads as running.
    pub fn has_running_execution(&self) -> bool {
        self.executions.iter().any(FlowExecution::is_active)
    }

    /// Begin cancelling the run `execution_id`, answering whether this flow could
    /// cancel it: a run that belongs to some OTHER flow, or to no flow at all, is not
    /// this flow's to cancel, and nor is one that has already finished.
    ///
    /// Call [`Flow::settle_due_executions`] first, so the judgement is made against the
    /// statuses the clock has already reached.
    pub fn begin_cancelling_execution(&mut self, execution_id: &str, now: SystemTime) -> bool {
        self.executions
            .iter_mut()
            .find(|execution| execution.execution_id == execution_id)
            .is_some_and(|execution| execution.begin_cancellation(now))
    }

    /// Begin cancelling every unfinished run of this flow, answering how many were
    /// eligible — what a cancellation that names no run at all does.
    pub fn begin_cancelling_all_executions(&mut self, now: SystemTime) -> usize {
        let mut cancelling = 0;
        for execution in &mut self.executions {
            if execution.begin_cancellation(now) {
                cancelling += 1;
            }
        }
        cancelling
    }

    /// This flow's runs, most recent first — the order `DescribeFlowExecutionRecords`
    /// reports them in.
    pub fn executions_newest_first(&self) -> Vec<&FlowExecution> {
        let mut ordered: Vec<&FlowExecution> = self.executions.iter().collect();
        ordered.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| b.execution_id.cmp(&a.execution_id))
        });
        ordered
    }

    /// The most recent run, or `None` when the flow has never run.
    ///
    /// Picks the maximum rather than ordering the whole history, so reading one flow's
    /// summary allocates nothing.
    pub fn last_run(&self) -> Option<&FlowExecution> {
        self.executions.iter().max_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.execution_id.cmp(&b.execution_id))
        })
    }

    /// `lastRunExecutionDetails`, derived from the most recent run so the detail and
    /// the run it summarises cannot disagree.
    ///
    /// `mostRecentExecutionMessage` is that run's own `executionMessage` — the one
    /// sentence the run is reported with wherever it is read, so a successful run says so
    /// here too rather than reading as empty
    /// ([`FlowExecution::execution_message`]).
    pub fn last_run_execution_details(&self) -> Option<ExecutionDetails> {
        let last = self.last_run()?;
        Some(ExecutionDetails {
            most_recent_execution_message: last.execution_message(),
            most_recent_execution_time: Some(to_date_time(last.last_updated_at)),
            most_recent_execution_status: Some(last.execution_status.as_str().to_string()),
        })
    }

    /// The connector type data flows FROM, as the stored source configuration names it.
    pub fn source_connector_type(&self) -> String {
        self.source_flow_config.connector_type.clone()
    }

    /// The connector type data flows TO, as the stored destination configuration names
    /// it. AppFlow transfers to one destination at a time, so the first entry is the
    /// flow's destination.
    pub fn destination_connector_type(&self) -> Option<String> {
        self.destination_flow_config_list
            .first()
            .map(|config| config.connector_type.clone())
    }

    /// The wire `FlowDefinition` for this flow — the summary `ListFlows` returns.
    pub fn to_flow_definition(&self) -> FlowDefinition {
        FlowDefinition {
            flow_arn: Some(self.flow_arn.clone()),
            description: self.description.clone(),
            flow_name: Some(self.flow_name.clone()),
            flow_status: Some(self.flow_status.as_str().to_string()),
            source_connector_type: Some(self.source_connector_type()),
            // A connector LABEL names a registered custom connector, which lives on a
            // connector profile; profile and connector registration are not part of
            // this slice, so there is nothing stored to label a flow with.
            source_connector_label: None,
            destination_connector_type: self.destination_connector_type(),
            destination_connector_label: None,
            trigger_type: Some(self.trigger_config.trigger_type.clone()),
            created_at: Some(to_date_time(self.created_at)),
            last_updated_at: Some(to_date_time(self.last_updated_at)),
            created_by: Some(self.created_by.clone()),
            last_updated_by: Some(self.last_updated_by.clone()),
            tags: Some(self.tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
            last_run_execution_details: self.last_run_execution_details(),
        }
    }
}

/// Per-service state for com-amazonaws-appflow-sandstoneconfigurationservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppflowSandstoneconfigurationservicelambdaState {
    /// Every flow in the account, keyed by `flowName` — which AppFlow treats as the
    /// flow's unique name. `BTreeMap`, so `ListFlows` pages in a stable order, and
    /// bounded by [`MAX_FLOWS_PER_ACCOUNT`].
    pub flows: BTreeMap<String, Flow>,
    /// Reserved for the connector-profile slice (`CreateConnectorProfile`,
    /// `DescribeConnectorProfiles`), which is still scaffolded — nothing writes this
    /// yet, and the slice that does must bound it the way [`Flow`] storage is bounded.
    pub connector_profiles: BTreeMap<String, crate::ops::types::ConnectorProfile>,
    /// Reserved for the connector-registration slice (`RegisterConnector`), which is
    /// still scaffolded. Keyed by `connectorLabel`.
    pub registered_connectors: BTreeMap<String, crate::ops::types::ConnectorDetail>,
    /// The key id of the AppFlow-managed KMS key this account's flows are encrypted
    /// with, minted on the first flow that names no key of its own (see
    /// [`Self::managed_kms_key_arn`]). ONE id for the account, so nothing a request can
    /// choose — the region it is signed for, the number of flows — grows this store.
    pub managed_kms_key_id: Option<String>,
}

impl ComAmazonawsAppflowSandstoneconfigurationservicelambdaState {
    /// The flow a repeated `CreateFlow` `@idempotencyToken` already stored, if any.
    ///
    /// Scans the flows, which [`MAX_FLOWS_PER_ACCOUNT`] bounds, instead of consulting a
    /// store-wide token map that nothing would ever reclaim.
    pub fn flow_for_client_token(&self, client_token: &str) -> Option<&Flow> {
        self.flows
            .values()
            .find(|flow| flow.client_token.as_deref() == Some(client_token))
    }

    /// The connector profile a flow definition names that this account does NOT hold, in
    /// the order the definition names them: its source first, then each of its
    /// destinations. `None` when every profile it names exists — which includes a
    /// definition that names none at all, because a connector AppFlow vends (Amazon S3,
    /// EventBridge) needs no profile to read from or write to.
    ///
    /// Shared by every operation that STORES a definition (`CreateFlow`, `UpdateFlow`), so
    /// the two cannot accept a flow the other would refuse; the answer borrows from the
    /// definition, which is what lets the refusal name the profile the request asked for.
    pub fn unresolved_connector_profile<'definition>(
        &self,
        source_flow_config: &'definition SourceFlowConfig,
        destination_flow_config_list: &'definition [DestinationFlowConfig],
    ) -> Option<&'definition str> {
        ::std::iter::once(source_flow_config.connector_profile_name.as_deref())
            .chain(
                destination_flow_config_list
                    .iter()
                    .map(|config| config.connector_profile_name.as_deref()),
            )
            .flatten()
            .find(|name| !self.connector_profiles.contains_key(*name))
    }

    /// The ARN of the AppFlow-managed KMS key for a flow in `region`, minting the key on
    /// first use.
    ///
    /// The key belongs to the ACCOUNT rather than to a flow, so it is minted once and
    /// then reported by every flow created without a key of its own: two flows cannot
    /// report two different managed keys, and a flow reports the same key every time it
    /// is read. See [`kms_key_arn`] for why the key id is opaque.
    pub fn managed_kms_key_arn(&mut self, region: &str) -> String {
        let key_id = self.managed_kms_key_id.get_or_insert_with(new_key_id);
        kms_key_arn(region, key_id)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppflowSandstoneconfigurationservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppflowSandstoneconfigurationservicelambdaState>)
}

/// A stored instant as the modelled `timestamp` the wire carries.
///
/// Converted with the SDK's own `From<SystemTime>`, which keeps the seconds and the
/// subsecond nanoseconds the instant was stored with — a round trip through `f64`
/// seconds would silently drop sub-microsecond resolution for a present-day epoch.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The region this request was signed for; [`DEFAULT_REGION`] when it was not signed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A flow's ARN: `arn:aws:appflow:<region>:<account>:flow/<flowName>`.
pub fn flow_arn(region: &str, flow_name: &str) -> String {
    ::emulator_server::emulator_arn("appflow", region, &format!("flow/{flow_name}"))
}

/// A registered connector's ARN:
/// `arn:aws:appflow:<region>:<account>:connector/<connectorLabel>`.
///
/// The label is what identifies one `ConnectorRegistration` in an account
/// (`smithy_query(query="doc", shape="DescribeConnectorRequest", member="connectorLabel")`),
/// so it is the resource part of the ARN, built the same way a flow's is.
pub fn connector_arn(region: &str, connector_label: &str) -> String {
    ::emulator_server::emulator_arn("appflow", region, &format!("connector/{connector_label}"))
}

/// One modelled shape as the JSON `Value` the service's own codec renders for it.
///
/// Rendering through the codec is what makes two shapes comparable at all: several of
/// them hold `std::collections::HashMap` members (`Task$taskProperties`, a connector's
/// `customProperties`, …) whose iteration order comes from a per-instance hash seed, so
/// neither `Debug` nor the rendered bytes of two equal values match. `serde_json::Value`
/// then supplies the equality: its objects compare by member, not by order, whichever
/// map backs them, so no local canonicalisation is needed — and lists keep their order,
/// which is right, because `tasks` and `destinationFlowConfigList` are sequences.
///
/// A value the codec cannot render answers `None` — the comparison is then impossible
/// rather than false, which the caller reports as a server fault instead of blaming the
/// request.
fn rendered_shape(
    schema: &::aws_smithy_schema::Schema,
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Option<::serde_json::Value> {
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer = ::aws_smithy_schema_server::json::JsonShapeSerializer::new();
    if let Err(error) = serializer.write_struct(schema, value) {
        ::tracing::warn!(
            %error,
            "the JSON codec could not render a stored shape, so two flow definitions cannot be compared"
        );
        return None;
    }
    let rendered = serializer.into_string();
    match ::serde_json::from_str::<::serde_json::Value>(&rendered) {
        Ok(parsed) => Some(parsed),
        Err(error) => {
            ::tracing::warn!(
                %error,
                "the JSON this service renders for a stored shape did not parse back"
            );
            None
        }
    }
}

/// The flow definition one `CreateFlow` carries, as one comparable value: every member
/// the operation persists, each modelled shape rendered by the service's own codec.
///
/// It exists for ONE comparison — the contract's "If you specify input parameters that
/// differ from your first request, an error occurs" — and `serde_json::Value`'s own
/// equality decides it: two definitions are equal exactly when every member is. Compared
/// in FULL rather than digested, so no collision can ever replay a success for a
/// definition that was never stored; it is built on demand for that one comparison,
/// never stored and never put on the wire, which is also why the modelled shapes need no
/// `Hash` or `PartialEq` of their own.
#[allow(clippy::too_many_arguments)]
pub fn definition_descriptor(
    flow_name: &str,
    description: Option<&str>,
    kms_arn: Option<&str>,
    trigger_config: &TriggerConfig,
    source_flow_config: &SourceFlowConfig,
    destination_flow_config_list: &[DestinationFlowConfig],
    tasks: &[Task],
    tags: &BTreeMap<String, String>,
    metadata_catalog_config: Option<&MetadataCatalogConfig>,
) -> Option<::serde_json::Value> {
    let destinations = destination_flow_config_list
        .iter()
        .map(|config| rendered_shape(&DestinationFlowConfig::SCHEMA, config))
        .collect::<Option<Vec<_>>>()?;
    let tasks = tasks
        .iter()
        .map(|task| rendered_shape(&Task::SCHEMA, task))
        .collect::<Option<Vec<_>>>()?;
    let trigger = rendered_shape(&TriggerConfig::SCHEMA, trigger_config)?;
    let source = rendered_shape(&SourceFlowConfig::SCHEMA, source_flow_config)?;
    let catalog = match metadata_catalog_config {
        Some(config) => Some(rendered_shape(&MetadataCatalogConfig::SCHEMA, config)?),
        None => None,
    };
    Some(::serde_json::json!({
        "flowName": flow_name,
        "description": description,
        "kmsArn": kms_arn,
        "tags": tags,
        "triggerConfig": trigger,
        "sourceFlowConfig": source,
        "destinationFlowConfigList": destinations,
        "tasks": tasks,
        "metadataCatalogConfig": catalog,
    }))
}

/// The task types whose source and destination fields ARE a source-to-destination
/// field mapping: `Map` maps one field onto another, `Map_all` maps every field
/// (`smithy_query(query="shape", shape="TaskType")`).
const MAPPING_TASK_TYPES: [&str; 2] = ["Map", "Map_all"];

/// The task properties that declare the DATA TYPE of a field
/// (`smithy_query(query="shape", shape="OperatorPropertiesKeys")`).
const DATA_TYPE_PROPERTIES: [&str; 3] = ["DATA_TYPE", "SOURCE_DATA_TYPE", "DESTINATION_DATA_TYPE"];

/// The task property that declares a flow's PARTITION KEYS, carried by a `Partition`
/// task (the same `OperatorPropertiesKeys` enum).
const PARTITION_KEYS_PROPERTY: &str = "ORDERED_PARTITION_KEYS_LIST";

/// The DATA SCHEMA a list of `tasks` declares, as one comparable value.
///
/// It is exactly the three settings `schemaVersion` counts a change to — the
/// source-to-destination field mappings, the field data types and the partition keys —
/// and nothing else, so a task that declares none of them (a filter, a validation)
/// contributes nothing and a flow that gains one keeps its schema version. The entries
/// are SORTED, because a mapping is a mapping wherever it sits in the list: re-ordering
/// the same tasks does not re-describe the data.
///
/// It exists for ONE comparison, and `serde_json::Value`'s own equality decides it:
/// two task lists declare the same schema exactly when every entry matches.
pub fn schema_descriptor(tasks: &[Task]) -> ::serde_json::Value {
    let mut entries: Vec<String> = Vec::new();
    for task in tasks {
        let mut entry = ::serde_json::Map::new();
        if MAPPING_TASK_TYPES.contains(&task.task_type.as_str()) {
            entry.insert(
                "mapping".to_string(),
                ::serde_json::json!({
                    "sourceFields": task.source_fields,
                    "destinationField": task.destination_field,
                }),
            );
        }
        let properties = task.task_properties.as_ref();
        let data_types: BTreeMap<&str, &str> = DATA_TYPE_PROPERTIES
            .iter()
            .filter_map(|key| {
                properties
                    .and_then(|properties| properties.get(*key))
                    .map(|declared| (*key, declared.as_str()))
            })
            .collect();
        if !data_types.is_empty() {
            entry.insert("dataTypes".to_string(), ::serde_json::json!(data_types));
        }
        if let Some(partition_keys) =
            properties.and_then(|properties| properties.get(PARTITION_KEYS_PROPERTY))
        {
            entry.insert(
                "partitionKeys".to_string(),
                ::serde_json::json!(partition_keys),
            );
        }
        if entry.is_empty() {
            continue;
        }
        // Rendered to a string so the entries can be ordered at all; `Map`'s own
        // ordering is already deterministic, so equal entries render equal.
        entries.push(::serde_json::Value::Object(entry).to_string());
    }
    entries.sort();
    ::serde_json::json!(entries)
}

/// A fresh run identifier. Opaque and unique, matching `ExecutionId`'s `^\S+$`
/// (`smithy_query(query="shape", shape="ExecutionId")`: len 0..256, `^\S+$`, which a
/// canonical hyphenated UUID satisfies).
///
/// The randomness is the `uuid` crate's v4 generator, reached through
/// `emulator_telemetry`'s uuid-backed id newtype (`BatchId::random()` is
/// `uuid::Uuid::new_v4()` rendered canonically) — a crate this service ALREADY depends
/// on — rather than by declaring a second path to the same crate. Nothing here is a
/// hand-rolled generator, and this service therefore adds no `Cargo.lock` entry, which
/// is what lets it land beside the other fixes merging into that one lock file
/// concurrently.
pub fn new_execution_id() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A fresh KMS key id: the opaque, canonically hyphenated UUID KMS mints for a key.
///
/// The same uuid-backed generator [`new_execution_id`] uses, for the same reason — an id
/// a service mints is opaque to its caller, and nothing here is a hand-rolled generator.
pub fn new_key_id() -> String {
    new_execution_id()
}

/// The smallest page the model allows a caller to ask for (`MaxResults`'s lower
/// `@range` bound, `smithy_query(query="shape", shape="MaxResults")`).
pub const MIN_PAGE_SIZE: usize = 1;

/// The largest page the model allows a caller to ask for (`MaxResults`'s upper
/// `@range` bound, `smithy_query(query="shape", shape="MaxResults")`).
pub const MAX_PAGE_SIZE: usize = 100;

/// The largest page a list operation returns when the caller names no `maxResults`.
///
/// An unbounded request is served the biggest page the model allows.
pub const DEFAULT_PAGE_SIZE: usize = MAX_PAGE_SIZE;

/// The page size to serve for a `maxResults`.
///
/// A value outside the modelled `MaxResults` range is REFUSED before this runs: the
/// framework checks `@range` at the request boundary and answers 400
/// `ValidationException`, so a `maxResults` of 0 or 101 never reaches a handler (pinned
/// by `tests/all/flow_lifecycle.rs`). The clamp here is therefore defence in depth, not
/// the enforcement: it guarantees the page this process builds is bounded whatever
/// value arrives, and it clamps to the SAME constants the model's bounds are recorded
/// in, so the two cannot drift apart.
pub fn page_size(max_results: Option<i32>) -> usize {
    page_size_or(max_results, DEFAULT_PAGE_SIZE)
}

/// The page size to serve for a `maxResults`, for a listing whose own documentation
/// states a default page of its own.
///
/// Same clamp as [`page_size`] — the two cannot bound a page differently — but the
/// caller names the page an absent `maxResults` means for THAT operation.
pub fn page_size_or(max_results: Option<i32>, default: usize) -> usize {
    match max_results {
        Some(requested) => (requested.max(0) as usize).clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
        None => default.clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE),
    }
}

/// Encode a pagination cursor: the `kind` of listing and the key the NEXT page starts
/// at, as one opaque token.
///
/// Encoded for OPACITY and shape — a client reads the token as a blob, and the standard
/// base64 alphabet (`A-Za-z0-9+/=`) carries no whitespace, so the token satisfies
/// `NextToken`'s `^\S+$` (`smithy_query(query="shape", shape="NextToken")`) whatever the
/// key contains. The codec is `aws_smithy_types::base64`, the one the wire serializer
/// already uses for every Smithy blob and already in this crate's dependency tree, so no
/// second encoder (and no `Cargo.lock` entry) is added for it.
///
/// It is deliberately NOT integrity-protected and claims no tamper resistance: base64 is
/// reversible, so a caller can mint one. Nothing downstream trusts it beyond choosing
/// where this account's own listing resumes, and a token another listing minted (or one
/// that does not decode) is refused rather than silently served from the start.
pub fn encode_cursor(kind: &str, start_at: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}\u{1f}{start_at}"))
}

/// The key a cursor resumes at, or `None` when the token was not minted by `kind`.
pub fn decode_cursor(kind: &str, token: &str) -> Option<String> {
    let decoded = ::aws_smithy_types::base64::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (token_kind, start_at) = decoded.split_once('\u{1f}')?;
    (token_kind == kind).then(|| start_at.to_string())
}

/// The listing a `DescribeConnectors` cursor belongs to.
pub const CONNECTOR_CURSOR: &str = "connectors";

/// The page `DescribeConnectors` serves a caller that names no `maxResults`
/// (`smithy_query(query="doc", shape="DescribeConnectorsRequest", member="maxResults")`:
/// "The maximum number of items that should be returned in the result set. The default
/// is 20.").
pub const DEFAULT_CONNECTOR_PAGE_SIZE: usize = 20;

/// The `connectorType` of a connector a caller registered themselves, rather than one
/// AppFlow vends (`smithy_query(query="shape", shape="ConnectorType")`).
pub const CONNECTOR_TYPE_CUSTOM: &str = "CustomConnector";

/// The connection mode a registered connector declares when a flow may READ from it.
///
/// `ConnectorMode` is a free-form string in the model
/// (`smithy_query(query="shape", shape="ConnectorMode")`); the two modes a connector
/// declares to AppFlow are the direction it supports — reading from the application and
/// writing to it (`ConnectorDetail$connectorModes`: "The connection mode that the
/// connector supports").
pub const CONNECTOR_MODE_SOURCE: &str = "SOURCE";

/// The connection mode a registered connector declares when a flow may WRITE to it (see
/// [`CONNECTOR_MODE_SOURCE`]).
pub const CONNECTOR_MODE_DESTINATION: &str = "DESTINATION";

/// Every scheduling frequency a scheduled flow can run at, in the order the model
/// declares them (`smithy_query(query="shape", shape="ScheduleFrequencyType")`).
pub const SCHEDULE_FREQUENCIES: [&str; 6] = [
    "BYMINUTE", "HOURLY", "DAILY", "WEEKLY", "MONTHLY", "ONCE",
];

/// Every operator a flow task can apply to a field it reads from a source, in the order
/// the model declares them (`smithy_query(query="shape", shape="Operators")`, the member
/// of `SupportedOperatorList`).
pub const OPERATORS: [&str; 21] = [
    "PROJECTION",
    "LESS_THAN",
    "GREATER_THAN",
    "CONTAINS",
    "BETWEEN",
    "LESS_THAN_OR_EQUAL_TO",
    "GREATER_THAN_OR_EQUAL_TO",
    "EQUAL_TO",
    "NOT_EQUAL_TO",
    "ADDITION",
    "MULTIPLICATION",
    "DIVISION",
    "SUBTRACTION",
    "MASK_ALL",
    "MASK_FIRST_N",
    "MASK_LAST_N",
    "VALIDATE_NON_NULL",
    "VALIDATE_NON_ZERO",
    "VALIDATE_NON_NEGATIVE",
    "VALIDATE_NUMERIC",
    "NO_OP",
];

/// What a destination that addresses an existing record accepts: every
/// `WriteOperationType`, in the order the model declares them
/// (`smithy_query(query="shape", shape="WriteOperationType")`).
const EVERY_WRITE_OPERATION: &[&str] = &["INSERT", "UPSERT", "UPDATE", "DELETE"];

/// One connector Amazon AppFlow vends, and what a flow may do with it.
///
/// This is service-wide, not per-account: the capabilities of a vended connector are
/// the same for every caller and no request changes them, which is why the catalog is a
/// table rather than a map in
/// [`ComAmazonawsAppflowSandstoneconfigurationservicelambdaState`]. The connectors a
/// CALLER registers are per-account and live in
/// [`ComAmazonawsAppflowSandstoneconfigurationservicelambdaState::registered_connectors`]
/// instead.
#[derive(Debug, Clone, Copy)]
pub struct VendedConnector {
    /// The `ConnectorType` this entry describes
    /// (`smithy_query(query="shape", shape="ConnectorType")`).
    pub connector_type: &'static str,
    /// What this connector is: the `connectorDescription` of its configuration ("A
    /// description about the connector",
    /// `smithy_query(query="doc", shape="ConnectorConfiguration",
    /// member="connectorDescription")`), which is the name AppFlow publishes for the
    /// application the connector reaches. Within `ConnectorDescription`'s `@length`
    /// 0..2048 and its `^[\w!@#\-.?,\s]*$` pattern.
    pub description: &'static str,
    /// Whether a flow can read from it.
    pub can_use_as_source: bool,
    /// Whether a flow can write to it.
    pub can_use_as_destination: bool,
    /// Whether a change in the source application can run a flow, which is what makes
    /// the `Event` trigger type available for it.
    pub supports_event_trigger: bool,
    /// Whether a connection to it can run over AWS PrivateLink.
    pub is_private_link_enabled: bool,
}

/// Every connector Amazon AppFlow vends.
///
/// The TYPES are the model's own `ConnectorType` enum, less `CustomConnector` (which
/// names a caller-registered connector, not a vended one), and they are listed in the
/// order the model declares them. The DESCRIPTION of each one is the name AppFlow
/// publishes for the application that connector reaches, as its list of supported
/// connectors names it. The CAPABILITIES are what AppFlow documents for each one: which
/// applications it supports as a flow's source and as a flow's destination, which of them
/// run a flow from an event in the source application, and which accept a connection over
/// AWS PrivateLink. Everything else a `ConnectorConfiguration` reports is DERIVED from
/// these (see [`VendedConnector::to_connector_configuration`]), so one entry cannot
/// claim a capability another part of the same answer denies.
///
/// A capability a connector DECLARES to AppFlow rather than one AppFlow implements for
/// it — the operators, write operations and data transfer types of
/// [`registered_connector_configuration`] — is no part of an entry here: a vended
/// connector declares nothing, so its configuration leaves those members unset.
pub const VENDED_CONNECTORS: [VendedConnector; 23] = [
    VendedConnector {
        connector_type: "Salesforce",
        description: "Salesforce",
        can_use_as_source: true,
        can_use_as_destination: true,
        // Salesforce change-data-capture events run a flow.
        supports_event_trigger: true,
        is_private_link_enabled: true,
    },
    VendedConnector {
        connector_type: "Singular",
        description: "Singular",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Slack",
        description: "Slack",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Redshift",
        description: "Amazon Redshift",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "S3",
        description: "Amazon S3",
        can_use_as_source: true,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Marketo",
        description: "Marketo",
        can_use_as_source: true,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Googleanalytics",
        description: "Google Analytics",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Zendesk",
        description: "Zendesk",
        can_use_as_source: true,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Servicenow",
        description: "ServiceNow",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Datadog",
        description: "Datadog",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Trendmicro",
        description: "Trend Micro",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Snowflake",
        description: "Snowflake",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Dynatrace",
        description: "Dynatrace",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Infornexus",
        description: "Infor Nexus",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Amplitude",
        description: "Amplitude",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Veeva",
        description: "Veeva",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "EventBridge",
        description: "Amazon EventBridge",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "LookoutMetrics",
        description: "Amazon Lookout for Metrics",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Upsolver",
        description: "Upsolver",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "Honeycode",
        description: "Amazon Honeycode",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "CustomerProfiles",
        description: "Amazon Connect Customer Profiles",
        can_use_as_source: false,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
    VendedConnector {
        connector_type: "SAPOData",
        description: "SAP OData",
        can_use_as_source: true,
        can_use_as_destination: true,
        supports_event_trigger: false,
        is_private_link_enabled: true,
    },
    VendedConnector {
        connector_type: "Pardot",
        description: "Salesforce Pardot",
        can_use_as_source: true,
        can_use_as_destination: false,
        supports_event_trigger: false,
        is_private_link_enabled: false,
    },
];

/// The connector AppFlow vends under `connector_type`, or `None` when no vended
/// connector has that type (`CustomConnector`, which names a registered connector).
pub fn vended_connector(connector_type: &str) -> Option<&'static VendedConnector> {
    VENDED_CONNECTORS
        .iter()
        .find(|connector| connector.connector_type == connector_type)
}

/// Every connector type a flow can transfer data TO, in the order AppFlow reports them.
///
/// The MEMBERS are the connector types a flow may name as its destination: every entry
/// of [`VENDED_CONNECTORS`] whose own record says a flow can write to it, plus
/// [`CONNECTOR_TYPE_CUSTOM`] — a flow can write to a connector an account registered
/// just as it can to one AppFlow vends, and that type is the one `ConnectorType` the
/// vended catalog has no entry for. [`destination_catalog_agrees`] is what keeps the two
/// from drifting: a connector is listed here exactly when its own entry says it can be a
/// destination.
///
/// The ORDER is the one AppFlow itself reports this list in, which is neither the order
/// the model declares `ConnectorType` in nor alphabetical; the values are the modelled
/// `ConnectorType` enum (`smithy_query(query="shape", shape="ConnectorTypeList")` — a
/// list over `ConnectorType`), so a destination a caller discovers here is a value
/// `CreateFlow` accepts for `destinationConnectorType`.
pub const DESTINATION_CONNECTORS: [&str; 13] = [
    "Salesforce",
    "Snowflake",
    "Redshift",
    "S3",
    "Honeycode",
    "LookoutMetrics",
    "CustomerProfiles",
    "Zendesk",
    "Marketo",
    CONNECTOR_TYPE_CUSTOM,
    "Upsolver",
    "SAPOData",
    "EventBridge",
];

/// `true` when [`DESTINATION_CONNECTORS`] lists exactly the connector types a flow can
/// write to: every vended connector whose entry says it can be a destination, none that
/// says it cannot, and the custom-connector type.
///
/// The two are stated separately — one entry per connector, one ordered list for the
/// answer — so this is what proves they agree; `tests/all/connector_configuration.rs`
/// asserts it.
pub fn destination_catalog_agrees() -> bool {
    let listed = |connector_type: &str| DESTINATION_CONNECTORS.contains(&connector_type);
    DESTINATION_CONNECTORS.len()
        == VENDED_CONNECTORS
            .iter()
            .filter(|connector| connector.can_use_as_destination)
            .count()
            + 1
        && listed(CONNECTOR_TYPE_CUSTOM)
        && VENDED_CONNECTORS
            .iter()
            .all(|connector| listed(connector.connector_type) == connector.can_use_as_destination)
}

/// Every connector type a flow can transfer data TO, as the wire reports it.
pub fn destination_connector_types() -> Vec<String> {
    DESTINATION_CONNECTORS
        .iter()
        .map(|connector_type| (*connector_type).to_string())
        .collect()
}

/// The `connectorMetadata` member that belongs to `connector_type`, naming which
/// connector the configuration describes.
///
/// `ConnectorMetadata` carries one member per connector AppFlow vends metadata for
/// (`smithy_query(query="shape", shape="ConnectorMetadata")`); a type it declares no
/// member for gets `None`.
fn connector_metadata(connector_type: &str) -> Option<ConnectorMetadata> {
    use crate::ops::types::{
        AmplitudeMetadata, CustomerProfilesMetadata, DatadogMetadata, DynatraceMetadata,
        EventBridgeMetadata, GoogleAnalyticsMetadata, HoneycodeMetadata, InforNexusMetadata,
        MarketoMetadata, PardotMetadata, RedshiftMetadata, S3Metadata, SAPODataMetadata,
        SalesforceMetadata, ServiceNowMetadata, SingularMetadata, SlackMetadata, SnowflakeMetadata,
        TrendmicroMetadata, UpsolverMetadata, VeevaMetadata, ZendeskMetadata,
    };

    let mut metadata = ConnectorMetadata::default();
    match connector_type {
        "Amplitude" => metadata.amplitude = Some(AmplitudeMetadata::default()),
        "CustomerProfiles" => metadata.customer_profiles = Some(CustomerProfilesMetadata::default()),
        "Datadog" => metadata.datadog = Some(DatadogMetadata::default()),
        "Dynatrace" => metadata.dynatrace = Some(DynatraceMetadata::default()),
        "EventBridge" => metadata.event_bridge = Some(EventBridgeMetadata::default()),
        "Googleanalytics" => metadata.google_analytics = Some(GoogleAnalyticsMetadata::default()),
        "Honeycode" => metadata.honeycode = Some(HoneycodeMetadata::default()),
        "Infornexus" => metadata.infor_nexus = Some(InforNexusMetadata::default()),
        "Marketo" => metadata.marketo = Some(MarketoMetadata::default()),
        "Pardot" => metadata.pardot = Some(PardotMetadata::default()),
        "Redshift" => metadata.redshift = Some(RedshiftMetadata::default()),
        "S3" => metadata.s3 = Some(S3Metadata::default()),
        "SAPOData" => metadata.sapo_data = Some(SAPODataMetadata::default()),
        "Salesforce" => metadata.salesforce = Some(SalesforceMetadata::default()),
        "Servicenow" => metadata.service_now = Some(ServiceNowMetadata::default()),
        "Singular" => metadata.singular = Some(SingularMetadata::default()),
        "Slack" => metadata.slack = Some(SlackMetadata::default()),
        "Snowflake" => metadata.snowflake = Some(SnowflakeMetadata::default()),
        "Trendmicro" => metadata.trendmicro = Some(TrendmicroMetadata::default()),
        "Upsolver" => metadata.upsolver = Some(UpsolverMetadata::default()),
        "Veeva" => metadata.veeva = Some(VeevaMetadata::default()),
        "Zendesk" => metadata.zendesk = Some(ZendeskMetadata::default()),
        // `LookoutMetrics` is the one vended connector `ConnectorMetadata` declares no
        // member for, so there is nothing to report for it.
        _ => return None,
    }
    Some(metadata)
}

/// Everything a `ConnectorConfiguration` states about what a FLOW may do with a
/// connector, DERIVED from that connector's capabilities alone.
///
/// Shared by the connectors AppFlow vends
/// ([`VendedConnector::to_connector_configuration`]) and the connectors a caller
/// registered ([`registered_connector_configuration`]), so the two can never report the
/// same capability differently:
///
/// * a connector a flow can read FROM is the one that decides how the flow runs, so it
///   reports the destinations a flow may send its records to, the frequencies a schedule
///   may use and the trigger types that can run the flow; a connector that can only be
///   written to reports none of those, because a flow is never driven by its
///   destination, and a member that does not apply to a connector is left UNSET rather
///   than reported as an empty list;
/// * what a connector DECLARES to AppFlow — the operators its tasks may apply, the write
///   operations it accepts and the data transfer types it moves — is no part of this:
///   those members belong to the record the capability came from, which is why only a
///   registered connector fills them in.
///
/// The members that IDENTIFY a connector (its type, its description, its metadata, the
/// registration it came from) are not capabilities, so each caller fills those in itself.
fn capability_configuration(
    can_use_as_source: bool,
    can_use_as_destination: bool,
    supports_event_trigger: bool,
    is_private_link_enabled: bool,
) -> ConnectorConfiguration {
    // In the order the model declares `TriggerType`.
    let mut supported_trigger_types: Vec<String> = Vec::new();
    if can_use_as_source {
        supported_trigger_types.push(TRIGGER_TYPE_SCHEDULED.to_string());
        if supports_event_trigger {
            supported_trigger_types.push(TRIGGER_TYPE_EVENT.to_string());
        }
        supported_trigger_types.push(TRIGGER_TYPE_ON_DEMAND.to_string());
    }

    ConnectorConfiguration {
        can_use_as_source,
        can_use_as_destination,
        supported_destination_connectors: can_use_as_source.then(destination_connector_types),
        supported_scheduling_frequencies: can_use_as_source.then(|| {
            SCHEDULE_FREQUENCIES
                .iter()
                .map(|frequency| (*frequency).to_string())
                .collect()
        }),
        is_private_link_enabled,
        // A connector resolves its own endpoint from the connector profile it is used
        // through, so the caller never supplies a PrivateLink endpoint URL for one.
        is_private_link_endpoint_url_required: false,
        supported_trigger_types: can_use_as_source.then_some(supported_trigger_types),
        ..ConnectorConfiguration::default()
    }
}

/// The wire `ConnectorConfiguration` for a connector a caller REGISTERED, built from the
/// registration the store holds.
///
/// A registration states which connection modes the connector supports, and those modes
/// are what a flow may do with it: one that declares the source mode can be read from,
/// one that declares the destination mode can be written to, and every derived member
/// then follows the same way it does for a connector AppFlow vends
/// ([`capability_configuration`]). A registered connector reaches its application
/// through the Lambda it was registered with rather than from AppFlow's own network, so
/// no PrivateLink connection and no application event belong to it; a destination it
/// serves accepts the write operations the connector interface defines, which is what
/// the Lambda implements, and it moves the data transfer types the registration declared.
pub fn registered_connector_configuration(
    detail: &crate::ops::types::ConnectorDetail,
    region: &str,
) -> ConnectorConfiguration {
    let declares = |mode: &str| {
        detail
            .connector_modes
            .as_ref()
            .is_some_and(|modes| modes.iter().any(|declared| declared == mode))
    };
    let can_use_as_source = declares(CONNECTOR_MODE_SOURCE);
    let can_use_as_destination = declares(CONNECTOR_MODE_DESTINATION);

    ConnectorConfiguration {
        connector_type: detail.connector_type.clone(),
        connector_label: detail.connector_label.clone(),
        connector_name: detail.connector_name.clone(),
        connector_description: detail.connector_description.clone(),
        connector_owner: detail.connector_owner.clone(),
        connector_version: detail.connector_version.clone(),
        connector_modes: detail.connector_modes.clone(),
        connector_provisioning_type: detail.connector_provisioning_type.clone(),
        registered_at: detail.registered_at,
        registered_by: detail.registered_by.clone(),
        connector_arn: detail
            .connector_label
            .as_deref()
            .map(|label| connector_arn(region, label)),
        // What the registration declares the connector can do, reported only where it
        // applies: the operators a task may apply to what a flow reads from it, the write
        // operations a flow may send to it, and the data transfer types it moves.
        supported_operators: can_use_as_source.then(|| {
            OPERATORS
                .iter()
                .map(|operator| (*operator).to_string())
                .collect()
        }),
        supported_write_operations: can_use_as_destination.then(|| {
            EVERY_WRITE_OPERATION
                .iter()
                .map(|operation| (*operation).to_string())
                .collect()
        }),
        supported_data_transfer_types: detail.supported_data_transfer_types.clone(),
        ..capability_configuration(
            can_use_as_source,
            can_use_as_destination,
            false,
            false,
        )
    }
}

impl VendedConnector {
    /// The wire `ConnectorConfiguration` for this connector: what a flow may do with it
    /// ([`capability_configuration`]), plus the members that name the connector the
    /// configuration describes.
    ///
    /// A vended connector is not registered by anyone, so it carries none of the
    /// registration members (`connectorLabel`, `connectorOwner`, `connectorVersion`,
    /// `connectorArn`, `registeredAt`, `registeredBy`, `connectorProvisioningType`,
    /// `connectorProvisioningConfig`) and none of the capabilities a registration
    /// declares (`supportedOperators`, `supportedWriteOperations`,
    /// `supportedDataTransferTypes`): those belong to a connector a caller registered
    /// ([`registered_connector_configuration`]).
    pub fn to_connector_configuration(&self) -> ConnectorConfiguration {
        ConnectorConfiguration {
            connector_type: Some(self.connector_type.to_string()),
            connector_description: Some(self.description.to_string()),
            connector_metadata: connector_metadata(self.connector_type),
            ..capability_configuration(
                self.can_use_as_source,
                self.can_use_as_destination,
                self.supports_event_trigger,
                self.is_private_link_enabled,
            )
        }
    }
}
