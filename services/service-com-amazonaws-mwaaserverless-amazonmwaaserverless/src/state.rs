// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mwaaserverless-amazonmwaaserverless service.
//!
//! The store holds the workflow control plane — the workflows a caller created, the
//! immutable versions of each one, and the runs started under them — plus the
//! idempotency ledger that lets a repeated `ClientToken` replay its first answer.
//! Every handler reads and writes THIS struct through [`service_state`]; nothing in
//! `src/ops/` keeps a store of its own.
//!
//! The store is PARTITIONED BY REGION: a workflow belongs to the endpoint it was
//! created through ([`request_region`], recorded as [`Workflow::region`]), and every
//! lookup, listing, name-uniqueness check and idempotency token is scoped to the
//! region of the calling request, because a regional AWS endpoint serves only the
//! resources created through it.

//! Every collection here is BOUNDED, because all three grow from caller requests:
//! `workflows` is capped at [`MAX_WORKFLOWS`], a workflow's `runs` at
//! [`MAX_RUNS_PER_WORKFLOW`] and its `versions` at [`MAX_VERSIONS_PER_WORKFLOW`] (a
//! create, a run start or an update past the ceiling is refused with the modelled
//! `ServiceQuotaExceededException`, and a workflow already holding that many runs
//! records no further automated one
//! ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::fire_due_scheduled_runs`]),
//! so the workflows x runs and workflows x versions
//! products are finite and so is the caller-supplied data each record holds); `workflow_names` holds exactly one
//! key per stored workflow, released when the delete's teardown removes the record
//! ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::reap_deleted_workflows`]);
//! a workflow's `tags` at [`MAX_TAGS_PER_RESOURCE`] (a tagging call past that ceiling is
//! refused and stores nothing); and the
//! idempotency ledger is capped and evicted on its own terms (see [`IdempotencyLedger`]).
//! A listing's response is bounded too, by the modelled `MaxResults` range.
//! A run's `task_instances` ([`TaskInstance`]) is NOT caller-driven growth: a task
//! instance exists because the run's definition DECLARED that task, so the map holds at
//! most one entry per declared task, which
//! [`workflow_definition::MAX_DECLARED_TASKS`] caps, and a run's plan
//! ([`workflow_execution::ExecutionPlan`]) holds one entry per task for the same reason.
//! The definition content a version freezes ([`WorkflowSnapshot::workflow_definition`])
//! is caller-driven data held per version, and is capped at
//! [`workflow_definition::MAX_DEFINITION_BYTES`] before a byte of the object is
//! buffered, so the versions x content product is finite too.
//! A run's `pending_log_lines` — the output it owes the log group its workflow configured
//! ([`run_logs`]) — is not caller-driven growth either: a line exists because a transition
//! of that run was materialised, so the queue holds at most the lines one execution
//! produces, which [`run_logs::MAX_PENDING_LOG_LINES`] caps, and the call that produced
//! them empties it.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{
    validate, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, Violation,
};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    Code, ConflictException, DefinitionS3Location, EncryptionConfiguration, LoggingConfiguration,
    NetworkConfiguration, ResourceNotFoundException, ScheduleConfiguration, ValidationException,
    ValidationExceptionField, ENCRYPTION_TYPE_VALUES,
};

/// Reading the workflow definition file a version snapshots out of Amazon S3, and the
/// schedule and tasks that definition declares.
pub mod workflow_definition;

/// Resolving the dependencies a workflow configuration names — its definition object and
/// its execution role — against the services that own them, before a version freezes it.
pub mod configuration_dependencies;

/// The task graph a run executes, built from the definition its version froze.
pub mod workflow_execution;

/// How long the work ONE task of a definition declares takes, read out of that
/// declaration.
pub mod declared_runtime;

/// Exporting a run's own execution output to the CloudWatch Logs log group its
/// workflow's `LoggingConfiguration` names.
pub mod run_logs;

/// The region used in minted ARNs when the request carries no usable SigV4
/// credential scope. Every generated emulator defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace (and SigV4 signing name) this service uses, per the model's
/// `awsService.arnNamespace`.
pub const ARN_NAMESPACE: &str = "airflow-serverless";

/// How long after `StartWorkflowRun` the run leaves `STARTING` for `QUEUED`: the
/// span the control plane's own admission of the run occupies. Deliberately
/// non-zero so a caller that reads the run straight back still observes the
/// `STARTING` its start reported.
pub const RUN_ADMITTED_AFTER: Duration = Duration::from_millis(400);

/// How long a run waits in `QUEUED` before an executor picks it up and it starts
/// `RUNNING`. The span is deliberately non-zero so a caller that polls immediately
/// still observes `QUEUED`.
pub const RUN_DEQUEUED_AFTER: Duration = Duration::from_millis(300);

/// How long ONE dependency level of a run's task graph occupies when nothing at that
/// level declares how long its work takes: the span those task instances are `RUNNING`
/// for.
///
/// The tasks at one level run together — nothing between them waits — so a level occupies
/// the longest span its own tasks run for
/// ([`workflow_execution::ExecutionPlan::level_runs_for`]), which is the wait each of them
/// declares ([`declared_runtime`]) and this default for each that declares none. A run
/// whose definition declares no task still occupies one. The length is the emulator's own
/// choice, not a fidelity claim about how long an undeclared task takes, and it is
/// deliberately non-zero so a caller that polls still observes `RUNNING` and the task
/// instances that are running in it.
pub const TASK_LEVEL_RUNS_FOR: Duration = Duration::from_millis(300);

/// `WorkflowStatus` as the model spells it
/// (`smithy_query(query="shape", shape="WorkflowStatus")` declares `READY | DELETING`).
///
/// "workflow is ready to run": the status a stored workflow reports until its
/// deletion starts.
pub const WORKFLOW_STATUS_READY: &str = "READY";

/// "workflow is being deleted": the status a workflow reports while the teardown
/// `DeleteWorkflow` started is still running. The ONLY other `WorkflowStatus` the
/// model declares, so a delete that removed the record synchronously would make it
/// unreachable.
pub const WORKFLOW_STATUS_DELETING: &str = "DELETING";

/// How long after `DeleteWorkflow` the workflow's teardown finishes and the record
/// disappears.
///
/// `DeleteWorkflow` is documented as a cleanup over the workflow's associated
/// resources — "stopping any running executions, removing scheduled triggers, and
/// cleaning up execution history" — which is the span `DELETING` covers. The length
/// is the emulator's own choice, not a fidelity claim about how long AWS takes, and
/// it is deliberately non-zero so a caller that describes or lists straight after
/// the delete still observes `DELETING` (and the workflow's name still taken) before
/// the record goes.
pub const WORKFLOW_DELETED_AFTER: Duration = Duration::from_millis(500);

/// `WorkflowRunStatus` values this slice uses.
///
/// The status a just-started run carries: "This is typically STARTING when you
/// first create the run" (`smithy_query(query="operation",
/// operation="StartWorkflowRun")`, `Status`).
pub const RUN_STATUS_STARTING: &str = "STARTING";
/// The status an admitted run waits in for an executor.
pub const RUN_STATUS_QUEUED: &str = "QUEUED";
/// The status a run carries while the tasks its definition declares are being
/// executed, between the executor picking it up and the last of those tasks
/// finishing.
pub const RUN_STATUS_RUNNING: &str = "RUNNING";
/// The modelled failure state a run that could not execute ends in.
pub const RUN_STATUS_FAILED: &str = "FAILED";
/// The status a run carries while the shutdown a stop started is still running:
/// "transitions the workflow run to a STOPPING state and then to STOPPED once all
/// cleanup is complete" (`smithy_query(query="operation",
/// operation="StopWorkflowRun")`).
pub const RUN_STATUS_STOPPING: &str = "STOPPING";
/// The status a run that was stopped reports once that shutdown is complete.
pub const RUN_STATUS_STOPPED: &str = "STOPPED";
/// The modelled success state: the status a run reaches once every task its
/// definition declared has finished successfully.
pub const RUN_STATUS_SUCCESS: &str = "SUCCESS";
/// The modelled timeout state. Terminal like the others, and not assigned here:
/// nothing in this emulator imposes a deadline on a run's execution, so no run of it
/// runs out of one.
pub const RUN_STATUS_TIMEOUT: &str = "TIMEOUT";

/// The `WorkflowRunStatus` values a run has FINISHED in and never leaves, out of the
/// eight the model declares (`smithy_query(query="shape",
/// shape="WorkflowRunStatus")`: the other four — `STARTING`, `QUEUED`, `RUNNING` and
/// `STOPPING` — are each on their way somewhere).
///
/// What is terminal is what decides whether there is still an execution for a stop to
/// end ([`WorkflowRun::begin_stop`]).
pub const RUN_TERMINAL_STATUSES: &[&str] = &[
    RUN_STATUS_SUCCESS,
    RUN_STATUS_FAILED,
    RUN_STATUS_TIMEOUT,
    RUN_STATUS_STOPPED,
];

/// How long after `StopWorkflowRun` the run leaves [`RUN_STATUS_STOPPING`] for
/// [`RUN_STATUS_STOPPED`]: the span the documented shutdown — stopping task
/// scheduling and terminating the active workers — occupies. The length is the
/// emulator's own choice, not a fidelity claim about how long AWS takes, and it is
/// deliberately non-zero so a caller that reads the run straight back still observes
/// the `STOPPING` its stop reported.
pub const RUN_STOPPED_AFTER: Duration = Duration::from_millis(400);

/// `RunType` for a run a caller started explicitly.
pub const RUN_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// `RunType` for a run the workflow's own trigger started, with no caller asking for
/// it — the other of the two values the model declares
/// (`smithy_query(query="shape", shape="RunType")`: `ON_DEMAND | SCHEDULED`).
pub const RUN_TYPE_SCHEDULED: &str = "SCHEDULED";

/// How long after a workflow's automated trigger is configured its FIRST firing falls
/// due, and the period between firings after that.
///
/// The cadence is the emulator's own choice, not a fidelity claim about any cron
/// expression: a workflow's declared schedule is reported as the definition writes it
/// ([`workflow_definition::declared_schedule`]), but nothing here evaluates that
/// expression into firing instants. It is deliberately non-zero, so a caller that lists
/// a freshly created workflow's runs still sees the empty history its create left
/// behind, and short, so a caller that waits sees the automated runs the trigger owes.
pub const SCHEDULED_RUN_EVERY: Duration = Duration::from_millis(500);

/// How many workflows this emulator will hold for the account.
///
/// The real service caps workflows per account with a service quota, and
/// `CreateWorkflow` models `ServiceQuotaExceededException` (HTTP 402) for exceeding it
/// ("This can occur when you attempt to create more workflows than allowed"). The number
/// here is NOT a fidelity claim about the published quota: it is the emulator's own
/// ceiling, set generously so no realistic workflow meets it, so that a caller looping on
/// create cannot grow this process's memory without bound.
pub const MAX_WORKFLOWS: usize = 1_000;

/// How many runs this emulator will hold for ONE workflow. Same reasoning, and
/// `StartWorkflowRun` models the same `ServiceQuotaExceededException` (the docs name
/// "concurrent workflow run limits" and "task execution limits" as its causes). With
/// [`MAX_WORKFLOWS`] this bounds the whole run store, including the caller-supplied
/// `OverrideParameters` each run holds.
pub const MAX_RUNS_PER_WORKFLOW: usize = 1_000;

/// How many owed firings ONE call materialises for a single workflow, and how many
/// across the whole store, before it stops and leaves the rest to the next call.
///
/// A trigger left behind owes one firing per [`SCHEDULED_RUN_EVERY`] of the gap, and
/// every handler materialises what the clock already owed before it answers
/// ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::advance_clock`]). Without a
/// per-call ceiling the work ONE request performs grows with the idle time before it
/// and with the number of triggers in the store — up to a whole store of runs built
/// under the lock every other request waits on. These ceilings bound that work per
/// call without skipping a firing: the trigger is left AT the firing a capped call
/// stopped at, so the next call continues the catch-up from there and every owed
/// firing still produces exactly one run. They are the emulator's own choice, set well
/// above the handful of firings a test or a caller that waited can owe.
pub const MAX_CATCHUP_FIRINGS_PER_WORKFLOW: usize = 16;

/// The whole-store companion of [`MAX_CATCHUP_FIRINGS_PER_WORKFLOW`].
pub const MAX_CATCHUP_FIRINGS_PER_CALL: usize = 128;

/// How many immutable versions this emulator will hold for ONE workflow.
///
/// `UpdateWorkflow` appends a version per call ("automatically creates a new version with
/// the updated configuration"), so the version list is caller-driven growth holding a
/// whole configuration snapshot each; past this ceiling an update is refused with the
/// `ServiceQuotaExceededException` the operation already models. Same reasoning as
/// [`MAX_WORKFLOWS`]: the number is the emulator's own ceiling, not a claim about the
/// published quota.
pub const MAX_VERSIONS_PER_WORKFLOW: usize = 1_000;

/// How many tags ONE resource of this service carries.
///
/// `Tags` is modelled `@length(min: 0, max: 50)`
/// (`smithy_query(query="shape", shape="Tags")`) wherever it appears — on a create, on a
/// tagging request and on the tag listing's response — so 50 is the size of the tag set a
/// resource of this service can report, and the ceiling a tagging call may not carry it
/// past.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most `@idempotencyToken` entries this service remembers.
///
/// Tokens are caller-supplied, so the ledger is caller-driven growth holding no resource
/// of its own: an entry naming a deleted workflow is dropped with it
/// ([`IdempotencyLedger::forget_workflow`]), and beyond this many live entries the oldest
/// is evicted ([`IdempotencyLedger::record`]). An evicted token is indistinguishable from
/// one whose idempotency window has passed: the next call carrying it creates a new
/// resource rather than replaying.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_024;

/// The definition and configuration a workflow version froze at the moment it was
/// created. A version is immutable, so a read of that version answers from here
/// rather than from the workflow's current configuration.
#[derive(Debug, Clone)]
pub struct WorkflowSnapshot {
    pub definition_s3_location: DefinitionS3Location,
    /// The definition CONTENT the object at [`Self::definition_s3_location`] held when
    /// this version was created — what `WorkflowDefinition` reports.
    ///
    /// The service "takes a snapshot of the definition at creation time, so subsequent
    /// changes to the Amazon S3 object will not affect the workflow unless you create a
    /// new version", so the content is read ONCE, here, and never re-read: a read of
    /// this version reports these bytes however the object has changed since. `None`
    /// when there was nothing to snapshot (see
    /// [`workflow_definition::read_definition`]).
    pub workflow_definition: Option<String>,
    /// The schedule the snapshotted definition declares — what `ScheduleConfiguration`
    /// reports. Derived from [`Self::workflow_definition`] when the version froze it, so
    /// it describes the same immutable definition.
    pub schedule_configuration: Option<ScheduleConfiguration>,
    pub code: Option<Code>,
    pub role_arn: String,
    pub description: Option<String>,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub logging_configuration: Option<LoggingConfiguration>,
    pub engine_version: Option<i32>,
    pub network_configuration: Option<NetworkConfiguration>,
    pub trigger_mode: Option<String>,
}

/// One immutable version of a workflow.
#[derive(Debug, Clone)]
pub struct WorkflowVersionRecord {
    pub version: String,
    pub created_at: SystemTime,
    pub is_latest: bool,
    pub snapshot: WorkflowSnapshot,
    /// Where this version falls in the order its workflow's versions were
    /// appended: the value of the monotonic mint counter
    /// ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::mint_seq`]) at the
    /// moment the version was recorded. It only ever grows, and it is not reused
    /// when a version is removed, so it is the POSITION a version listing pages
    /// over ([`format_version_listing_key`]): a cursor carrying it still resolves
    /// to the same place in the remaining history after any other version has
    /// gone.
    pub sequence: u64,
}

impl WorkflowVersionRecord {
    /// The instant this version's code artifacts were copied, and `None` for a
    /// version that froze no code location at all.
    ///
    /// A `Code` location is copied "at the time of the request"
    /// (`smithy_query(query="doc", shape="CreateWorkflowRequest", member="Code")`)
    /// that supplied it, and a version is immutable, so the copy instant of the
    /// artifacts a version reports IS that version's creation instant. A version
    /// that supplied no location had nothing copied, so it has no such instant to
    /// report — the same absence it reports for the location itself.
    pub fn code_snapshotted_at(&self) -> Option<SystemTime> {
        self.snapshot.code.as_ref().map(|_| self.created_at)
    }
}

/// The CloudWatch Logs log group a version's configuration names as the destination of
/// its runs' output, and `None` for a version that configures no logging.
///
/// It is `LoggingConfiguration.LogGroupName`, "the name of the CloudWatch log group where
/// workflow execution logs are stored" — the one place the destination is read out of a
/// snapshot, so a run that was started and a run a trigger fired export to the same group.
pub fn configured_log_group(snapshot: &WorkflowSnapshot) -> Option<String> {
    snapshot
        .logging_configuration
        .as_ref()
        .map(|configuration| configuration.log_group_name.clone())
        .filter(|name| !name.is_empty())
}

/// The fixed text in front of the log group this service names for a workflow whose
/// create or update supplied no `LoggingConfiguration` of its own.
///
/// Recorded behaviour: the group the service generates is this prefix, the workflow's own
/// id — the resource-id segment of its ARN ([`workflow_resource_id`]) — and a trailing
/// `/`. Fixture `mwaa-serverless-workflow-run-two-dependent-tasks-succeed` reports
/// `GetWorkflow`'s `LoggingConfiguration.LogGroupName` in exactly that form for a create
/// that named none.
const SERVICE_LOG_GROUP_PREFIX: &str = "/aws/mwaa-serverless/";

/// The log group THIS SERVICE names for the runs of the workflow `workflow_arn`:
/// [`SERVICE_LOG_GROUP_PREFIX`], the workflow's own id and a trailing `/`.
///
/// Every character of it is one a CloudWatch Logs log group name may carry
/// ([`is_log_group_name`]) — the id segment the modelled `WorkflowArn` pattern allows is
/// alphanumerics, `.`, `-` and `_` — so the group it names is one this service's run-log
/// export can really create ([`run_logs`]).
pub fn service_log_group_name(workflow_arn: &str) -> String {
    format!(
        "{SERVICE_LOG_GROUP_PREFIX}{}/",
        workflow_resource_id(workflow_arn)
    )
}

/// The `TriggerMode` a workflow reports when its create or update named none, for a
/// version whose snapshotted definition declares NO schedule: its runs are the ones a
/// caller starts, and nothing else starts them.
///
/// Recorded behaviour: the fixture above creates a workflow from a definition declaring
/// `schedule: null` with no `TriggerMode` in the request, and `GetWorkflow` reports this
/// token.
pub const TRIGGER_MODE_MANUAL_ONLY: &str = "manual_only";

/// The companion of [`TRIGGER_MODE_MANUAL_ONLY`] for a version whose definition DOES
/// declare a schedule, which is the only other side of the same derivation: the service
/// configures that definition's trigger, so the workflow's runs are not manual-only.
///
/// Only the manual-only token is recorded, so this spelling is the emulator's own for the
/// case the recording does not cover; what is NOT the emulator's own is the derivation,
/// which is the rule the recorded case establishes.
pub const TRIGGER_MODE_SCHEDULED: &str = "scheduled";

/// The `LoggingConfiguration` a version FREEZES: the one the request supplied, or the
/// group this service names for the workflow when the request supplied none
/// ([`service_log_group_name`]).
///
/// The service defaults the member at create and at update time, so every reader —
/// `GetWorkflow`, `ListWorkflows`, `ListWorkflowVersions` and the run-log export that
/// reads the destination out of the snapshot ([`configured_log_group`]) — reports the
/// value the service applied rather than nothing.
pub fn applied_logging_configuration(
    supplied: Option<&LoggingConfiguration>,
    workflow_arn: &str,
) -> LoggingConfiguration {
    match supplied {
        Some(configuration) if !configuration.log_group_name.is_empty() => configuration.clone(),
        _ => LoggingConfiguration {
            log_group_name: service_log_group_name(workflow_arn),
        },
    }
}

/// The `TriggerMode` a version FREEZES: the one the request supplied, or the mode the
/// service derives from the schedule that version's snapshotted definition declares when
/// the request supplied none.
///
/// The derivation is the definition's, because the definition is where the service reads
/// scheduling information from — "when a workflow includes schedule information in its
/// YAML definition, the service automatically configures the appropriate triggers for
/// automated execution" (`smithy_query(query="doc", shape="GetWorkflowResponse",
/// member="ScheduleConfiguration")`) — so a definition declaring no schedule leaves the
/// workflow's runs to a caller ([`TRIGGER_MODE_MANUAL_ONLY`]) and one declaring a schedule
/// does not ([`TRIGGER_MODE_SCHEDULED`]).
pub fn applied_trigger_mode(
    supplied: Option<&str>,
    schedule_configuration: Option<&ScheduleConfiguration>,
) -> String {
    match supplied {
        Some(mode) if !mode.is_empty() => mode.to_string(),
        _ if schedule_configuration.is_some() => TRIGGER_MODE_SCHEDULED.to_string(),
        _ => TRIGGER_MODE_MANUAL_ONLY.to_string(),
    }
}

/// The key the service's OWN entry in a run's parameter set is reported under.
///
/// Recorded behaviour: fixture
/// `mwaa-serverless-workflow-run-two-dependent-tasks-succeed` starts a run carrying no
/// `OverrideParameters` at all, and `GetWorkflowRun` reports a map holding this key
/// against the workflow's own id.
pub const RUN_WORKFLOW_ID_PARAMETER: &str = "mwaa_serverless_workflow_id";

/// The parameter set a run of `workflow_arn` executes with: the service-injected entry
/// naming the workflow the run belongs to ([`RUN_WORKFLOW_ID_PARAMETER`]), with the
/// caller's own overrides merged over it.
///
/// Seeded first, so a caller that supplies the same key overrides it the way it overrides
/// any other parameter, and reported by `GetWorkflowRun` out of the same map whatever
/// started the run — a caller's `StartWorkflowRun` or the workflow's own trigger.
pub fn run_parameters<I>(
    workflow_arn: &str,
    supplied: I,
) -> BTreeMap<String, ::aws_smithy_types::Document>
where
    I: IntoIterator<Item = (String, ::aws_smithy_types::Document)>,
{
    let mut parameters = BTreeMap::new();
    parameters.insert(
        RUN_WORKFLOW_ID_PARAMETER.to_string(),
        ::aws_smithy_types::Document::String(workflow_resource_id(workflow_arn).to_string()),
    );
    parameters.extend(supplied);
    parameters
}

/// A run started under a workflow.
#[derive(Debug, Clone)]
pub struct WorkflowRun {
    pub run_id: String,
    pub workflow_arn: String,
    pub workflow_version: String,
    pub run_type: String,
    pub status: String,
    pub override_parameters: BTreeMap<String, ::aws_smithy_types::Document>,
    pub created_at: SystemTime,
    pub started_on: SystemTime,
    pub completed_on: Option<SystemTime>,
    pub modified_at: SystemTime,
    pub duration: Option<i32>,
    pub error_message: Option<String>,
    /// The task instances this run produced, keyed by `TaskInstanceId` — a
    /// `BTreeMap` so a reader walks them in one total order. One per task the
    /// run's definition declared, recorded when the run starts executing them
    /// ([`WorkflowRun::settle`]); a run whose definition declared none, and one
    /// whose graph could not be built at all, hold none, so `GetTaskInstance`
    /// answers the modelled `ResourceNotFoundException` rather than reporting a
    /// task the definition never declared.
    pub task_instances: BTreeMap<String, TaskInstance>,
    /// What this run executes: the task graph built from the definition its
    /// version froze, once, when the run was recorded
    /// ([`workflow_execution::ExecutionPlan`]). A version is immutable, so what it
    /// holds IS what this run executes, and a plan built once is what makes two
    /// reads of the same run answer identically.
    pub execution: workflow_execution::ExecutionPlan,
    /// The instant the clock owes this run's NEXT status change. Stored, never
    /// slept on, and advanced by [`WorkflowRun::settle`] as each change lands.
    pub next_status_at: SystemTime,
    /// Where this run's execution output is EXPORTED to: the CloudWatch Logs log
    /// group named by the `LoggingConfiguration` of the version it runs, frozen when
    /// the run was recorded the way its execution plan is ([`run_logs`]). `None` for
    /// a run whose version configures no logging: nothing documents a destination for
    /// its output, so none is exported and its task instances name no `LogStream`.
    pub log_group_name: Option<String>,
    /// The lines this run has produced and not yet delivered to that group. Filled as
    /// each transition is materialised and emptied by the call that materialised it,
    /// once that call has released this store ([`WorkflowRun::take_pending_log_lines`],
    /// [`run_logs::publish`]), so this service's store and CloudWatch Logs' are never
    /// locked at once. Bounded by [`run_logs::MAX_PENDING_LOG_LINES`], which holds
    /// every line one run's whole execution can produce.
    pub pending_log_lines: Vec<run_logs::LogLine>,
    /// Where this run falls in the store's creation order: the value of the
    /// monotonic mint counter ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::mint_seq`])
    /// at the moment the run was recorded. It breaks a tie between two runs the
    /// clock cannot separate, so [`WorkflowRun::listing_key`] is a TOTAL order
    /// that still only ever grows as runs are created.
    pub sequence: u64,
}

/// What advancing a run's execution by one dependency level left it in
/// ([`WorkflowRun::advance_declared_tasks`]).
#[derive(Debug, Clone)]
enum LevelOutcome {
    /// A level has started, and occupies this span.
    Running(Duration),
    /// No level was left to start: every task the definition declared finished, and
    /// none of them failed.
    Succeeded,
    /// A task of the level that just ended declared work that cannot succeed, so the
    /// execution is over; this is the reason the run reports.
    Failed(String),
}

/// The position of a run in the order a run listing walks: its creation instant
/// (nanoseconds since the epoch) and, for two runs created at the same instant,
/// the creation sequence that separates them.
///
/// Ordering runs by this key — rather than by the identifier they are stored
/// under — is what makes a listing chronological, and what keeps a run created
/// after a page was served sorting AFTER that page's cursor instead of before it.
pub type RunListingKey = (u128, u64);

impl WorkflowRun {
    /// This run's position in the listing order (see [`RunListingKey`]).
    pub fn listing_key(&self) -> RunListingKey {
        (
            self.created_at
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            self.sequence,
        )
    }

    /// The `TaskInstanceId`s this run holds, in the store's order — what
    /// `GetWorkflowRun` reports as `RunDetail.TaskInstances`.
    pub fn task_instance_ids(&self) -> Vec<String> {
        self.task_instances.keys().cloned().collect()
    }

    /// Apply EVERY status change the clock has already made due, and report
    /// whether anything changed.
    ///
    /// This is the ONLY write a reporting operation may make: at `now` each
    /// transition below was owed whether or not anybody called, so the status a
    /// reader reports is the one the run would have reached unpolled. Every
    /// timestamp it writes comes from the DUE instant, never from `now`, so two
    /// reads after the same deadline answer identically.
    ///
    /// The schedule is the lifecycle the control plane itself drives, and the
    /// execution the run's plan lays out ([`workflow_execution::ExecutionPlan`]):
    ///
    /// * `STARTING` -> `QUEUED` after [`RUN_ADMITTED_AFTER`]: the run has been
    ///   admitted and is waiting for an executor.
    /// * `QUEUED` -> `RUNNING` after [`RUN_DEQUEUED_AFTER`]: an executor has picked
    ///   the run up, so the tasks its definition declared start — one task instance
    ///   per declared task, the ones that wait for nothing first.
    /// * `QUEUED` -> `FAILED` instead, for a definition whose task graph could not
    ///   be built: that is a definition no run of it can execute, so the run fails
    ///   with the reason the plan recorded and holds no task instance.
    /// * `RUNNING` -> `RUNNING`, one dependency level at a time: the level that was
    ///   running finishes once the span it occupies is up — the longest span its own
    ///   tasks declare ([`workflow_execution::ExecutionPlan::level_runs_for`]) — and the
    ///   level that waited for it starts, for the span ITS tasks declare.
    /// * `RUNNING` -> `SUCCESS` once no level is left: every task the definition
    ///   declared has finished, so the run is complete and carries the completion
    ///   instant and duration every finished run does, and no `ErrorMessage` —
    ///   "This value is null if the run completed successfully". The window
    ///   `StartedOn`/`CompletedOn` then span, and the `Duration` derived from it, are
    ///   therefore the window the declared work occupies.
    /// * `RUNNING` -> `FAILED` instead, as soon as a level ends with a task whose own
    ///   declaration states work that cannot succeed ([`workflow_execution::PlannedTask`]):
    ///   that instance ends `FAILED`, no task that was waiting on the execution runs
    ///   (`UPSTREAM_FAILED`), and the run carries the reason it failed on as its
    ///   `ErrorMessage` beside the completion instant and duration.
    /// * `STOPPING` -> `STOPPED` after [`RUN_STOPPED_AFTER`]: the shutdown a stop
    ///   started is complete, so the run is finished and carries the completion
    ///   instant and duration every finished run does. It records no `ErrorMessage`:
    ///   a run the caller stopped did not fail. The stop "terminates all running
    ///   tasks", so the instances it was still running end `CANCELLED`.
    ///
    /// A terminal status is never left: the loop stops at any status no
    /// transition is owed from, which is what makes a settled run stable across
    /// reads.
    ///
    /// Every status change it applies, and every task instance it starts or ends, is
    /// also a line the run owes the log group its workflow configured, queued here as
    /// the change is made ([`run_logs`]): the output a run produces is produced by the
    /// execution being materialised, so the two cannot come apart.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let mut changed = false;
        while now >= self.next_status_at {
            let due = self.next_status_at;
            match self.status.as_str() {
                RUN_STATUS_STARTING => {
                    self.status = RUN_STATUS_QUEUED.to_string();
                    self.modified_at = due;
                    self.next_status_at = deadline_after(due, RUN_DEQUEUED_AFTER);
                    self.log_status_reached(due);
                }
                RUN_STATUS_QUEUED => {
                    self.modified_at = due;
                    match self.execution.build_error.clone() {
                        Some(reason) => {
                            self.status = RUN_STATUS_FAILED.to_string();
                            self.complete_at(due);
                            self.error_message = Some(reason);
                            self.log_status_reached(due);
                        }
                        None => {
                            self.status = RUN_STATUS_RUNNING.to_string();
                            self.log_status_reached(due);
                            self.start_declared_tasks(due);
                            // The level now running occupies the span its own tasks
                            // declare.
                            self.next_status_at =
                                deadline_after(due, self.execution.level_runs_for(0));
                        }
                    }
                }
                RUN_STATUS_RUNNING => {
                    self.modified_at = due;
                    match self.advance_declared_tasks(due) {
                        LevelOutcome::Running(runs_for) => {
                            self.next_status_at = deadline_after(due, runs_for);
                        }
                        LevelOutcome::Succeeded => {
                            self.status = RUN_STATUS_SUCCESS.to_string();
                            self.complete_at(due);
                            self.log_status_reached(due);
                        }
                        LevelOutcome::Failed(reason) => {
                            self.status = RUN_STATUS_FAILED.to_string();
                            self.complete_at(due);
                            self.error_message = Some(reason);
                            self.log_status_reached(due);
                        }
                    }
                }
                RUN_STATUS_STOPPING => {
                    self.status = RUN_STATUS_STOPPED.to_string();
                    self.modified_at = due;
                    self.complete_at(due);
                    self.cancel_unfinished_tasks(due);
                    self.log_status_reached(due);
                }
                // Terminal, or a status whose exit no clock owes.
                _ => break,
            }
            changed = true;
        }
        changed
    }

    /// Queue one line for the stream `stream` at `at`, when this run has a log group to
    /// export to ([`Self::log_group_name`]) and has not already queued every line one
    /// run can owe ([`run_logs::MAX_PENDING_LOG_LINES`]).
    fn queue_log_line(&mut self, stream: String, at: SystemTime, message: String) {
        let Some(log_group) = self.log_group_name.clone() else {
            return;
        };
        if self.pending_log_lines.len() >= run_logs::MAX_PENDING_LOG_LINES {
            return;
        }
        self.pending_log_lines.push(run_logs::LogLine {
            log_group,
            log_stream: stream,
            at,
            message,
        });
    }

    /// Take the lines this run owes its log group, leaving none behind: what the
    /// calling handler delivers once it has released this store
    /// ([`run_logs::publish`]).
    pub fn take_pending_log_lines(&mut self) -> Vec<run_logs::LogLine> {
        ::std::mem::take(&mut self.pending_log_lines)
    }

    /// Queue the line this run's own stream owes for having been ADMITTED, dated from
    /// the instant it was created.
    ///
    /// Called by each of the two places a run enters the store — a caller's
    /// `StartWorkflowRun` and a workflow's own trigger
    /// ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::fire_due_scheduled_runs`])
    /// — so a run nothing started exports nothing.
    pub fn log_admitted(&mut self) {
        let stream = run_logs::run_log_stream(&self.run_id);
        let message = format!(
            "Workflow run {} of workflow {} version {} was accepted as a {} run.",
            self.run_id, self.workflow_arn, self.workflow_version, self.run_type
        );
        let at = self.created_at;
        self.queue_log_line(stream, at, message);
    }

    /// Queue the line this run's own stream owes for the status it has JUST reached:
    /// the status itself while the run is still on its way somewhere, and the outcome
    /// it finished with — the span it occupied and, for a run that failed, the reason
    /// it reports — once it has reached a status it never leaves.
    fn log_status_reached(&mut self, at: SystemTime) {
        let stream = run_logs::run_log_stream(&self.run_id);
        if !self.is_terminal() {
            let message = format!(
                "Workflow run {} entered status {}.",
                self.run_id, self.status
            );
            self.queue_log_line(stream, at, message);
            return;
        }
        let message = format!(
            "Workflow run {} finished with status {} after {} seconds.",
            self.run_id,
            self.status,
            self.duration.unwrap_or_default()
        );
        self.queue_log_line(stream.clone(), at, message);
        if let Some(reason) = self.error_message.clone() {
            let message = format!("Workflow run {} failed: {reason}", self.run_id);
            self.queue_log_line(stream, at, message);
        }
    }

    /// Record this run as COMPLETE at `at`: the completion instant and the duration
    /// every finished run carries, computed over the span the run occupied.
    fn complete_at(&mut self, at: SystemTime) {
        let elapsed = at.duration_since(self.started_on).unwrap_or_default();
        self.completed_on = Some(at);
        self.duration = Some(duration_in_seconds(elapsed));
    }

    /// Record one task instance per task the run's plan holds and start the first
    /// dependency level: the tasks that wait for nothing are `RUNNING` from `at`, and
    /// every other task is `SCHEDULED` until the level it waits for is done.
    ///
    /// An instance already stored under a planned identifier is left alone, so this
    /// writes each instance once however often the clock is advanced.
    ///
    /// An instance that STARTS here names the log stream its attempt writes to
    /// ([`run_logs::task_log_stream`]) and owes that stream the line saying it started;
    /// one that is still waiting names none, because no execution of it has written
    /// anything yet — `LogStream` is "the CloudWatch log stream name for this task
    /// instance execution".
    fn start_declared_tasks(&mut self, at: SystemTime) {
        let run_id = self.run_id.clone();
        let exporting = self.log_group_name.is_some();
        // The lines the instances started here owe, collected while the plan is
        // borrowed and queued once it is not.
        let mut lines: Vec<(String, String)> = Vec::new();
        for task in &self.execution.tasks {
            // The run executes a declared task once, so its instance is the first
            // attempt at it, and the identifier that attempt is recorded under is the
            // one minted for it (`planned_instance_id`).
            let attempt_number = FIRST_ATTEMPT_NUMBER;
            let task_instance_id = planned_instance_id(&run_id, task);
            if self.task_instances.contains_key(&task_instance_id) {
                continue;
            }
            let running = task.level == 0;
            let log_stream = (running && exporting)
                .then(|| run_logs::task_log_stream(&run_id, &task_instance_id, attempt_number));
            let instance = TaskInstance {
                task_instance_id: task_instance_id.clone(),
                task_id: Some(task.task_id.clone()),
                status: if running {
                    TASK_INSTANCE_STATUS_RUNNING.to_string()
                } else {
                    TASK_INSTANCE_STATUS_SCHEDULED.to_string()
                },
                operator_name: task.operator_name.clone(),
                attempt_number: Some(attempt_number),
                started_at: running.then_some(at),
                ended_at: None,
                modified_at: at,
                error_message: None,
                // The stream this attempt's output goes to, for an attempt that has
                // started; nothing ran an operator that could push a value to another
                // task.
                log_stream: log_stream.clone(),
                xcom: BTreeMap::new(),
            };
            if let Some(stream) = log_stream {
                lines.push((stream, task_started_message(&instance)));
            }
            self.task_instances.insert(task_instance_id, instance);
        }
        for (stream, message) in lines {
            self.queue_log_line(stream, at, message);
        }
    }

    /// Advance the run's execution by ONE dependency level at `at`: the level that was
    /// running ends with the outcome each of its tasks declared, and the level that waited
    /// for it starts.
    ///
    /// Reports how long the level that just STARTED occupies — the longest span its own
    /// tasks declare ([`workflow_execution::ExecutionPlan::level_runs_for`]) — and
    /// [`LevelOutcome::Succeeded`] once the execution is COMPLETE: no level was left to
    /// start, which is also the answer for a run whose definition declared no task at all.
    ///
    /// A level in which any instance ended `FAILED` ends the EXECUTION
    /// ([`LevelOutcome::Failed`]): the tasks that were waiting on it never run, so they
    /// end `UPSTREAM_FAILED` and no further level is started.
    ///
    /// Each attempt that ends owes its own stream the outcome it ended with, and each
    /// attempt that starts names the stream it writes to and owes it the line saying so
    /// ([`Self::start_declared_tasks`]).
    fn advance_declared_tasks(&mut self, at: SystemTime) -> LevelOutcome {
        let Some(level) = self.running_level() else {
            return LevelOutcome::Succeeded;
        };
        let run_id = self.run_id.clone();
        let exporting = self.log_group_name.is_some();
        let mut lines: Vec<(String, String)> = Vec::new();
        // The reason the run reports, when a task of this level declared work that cannot
        // succeed: the first such task in the plan's order, so two reads answer alike.
        let mut failed_on: Option<String> = None;
        for task in self.execution.tasks_at(level) {
            if let Some(reason) = task.failure.clone() {
                failed_on = Some(reason);
                break;
            }
        }
        // The instances of this level, each with the outcome its own task declared,
        // collected while the plan is borrowed and applied once it is not.
        let ending: Vec<(String, Option<String>)> = self
            .execution
            .tasks_at(level)
            .map(|task| (planned_instance_id(&run_id, task), task.failure.clone()))
            .collect();
        for (id, failure) in ending {
            if let Some(instance) = self.task_instances.get_mut(&id) {
                instance.status = match &failure {
                    Some(_) => TASK_INSTANCE_STATUS_FAILED.to_string(),
                    None => TASK_INSTANCE_STATUS_SUCCESS.to_string(),
                };
                instance.error_message = failure;
                instance.ended_at = Some(at);
                instance.modified_at = at;
                if let Some(stream) = instance.log_stream.clone() {
                    lines.push((stream, task_finished_message(instance)));
                }
            }
        }
        // A failed task is work the tasks after it waited for and never got, so nothing
        // deeper in the graph is executed at all.
        if let Some(reason) = failed_on {
            self.fail_waiting_tasks(at);
            for (stream, message) in lines {
                self.queue_log_line(stream, at, message);
            }
            return LevelOutcome::Failed(reason);
        }
        let next = self.task_instance_ids_at(level + 1);
        if next.is_empty() {
            for (stream, message) in lines {
                self.queue_log_line(stream, at, message);
            }
            return LevelOutcome::Succeeded;
        }
        for id in next {
            if let Some(instance) = self.task_instances.get_mut(&id) {
                instance.status = TASK_INSTANCE_STATUS_RUNNING.to_string();
                instance.started_at = Some(at);
                instance.modified_at = at;
                let stream = exporting.then(|| task_log_stream_of(&run_id, instance));
                instance.log_stream = stream.clone();
                if let Some(stream) = stream {
                    lines.push((stream, task_started_message(instance)));
                }
            }
        }
        for (stream, message) in lines {
            self.queue_log_line(stream, at, message);
        }
        LevelOutcome::Running(self.execution.level_runs_for(level + 1))
    }

    /// End every instance still WAITING at `at` as `UPSTREAM_FAILED`: a task the
    /// execution failed before reaching never ran, so it reports neither an outcome of its
    /// own nor a window.
    ///
    /// Only the waiting status this emulator's own execution assigns is ended, so an
    /// instance that had already finished keeps the outcome it finished with.
    fn fail_waiting_tasks(&mut self, at: SystemTime) {
        for instance in self.task_instances.values_mut() {
            if instance.status != TASK_INSTANCE_STATUS_SCHEDULED {
                continue;
            }
            instance.status = TASK_INSTANCE_STATUS_UPSTREAM_FAILED.to_string();
            instance.modified_at = at;
        }
    }

    /// The dependency level the run is executing: the shallowest level whose task
    /// instances are still `RUNNING`, and `None` when none of them is.
    fn running_level(&self) -> Option<usize> {
        self.execution
            .tasks
            .iter()
            .filter(|task| {
                self.task_instances
                    .get(&planned_instance_id(&self.run_id, task))
                    .is_some_and(|instance| instance.status == TASK_INSTANCE_STATUS_RUNNING)
            })
            .map(|task| task.level)
            .min()
    }

    /// The identifiers of the instances of the planned tasks at `level`.
    fn task_instance_ids_at(&self, level: usize) -> Vec<String> {
        self.execution
            .tasks_at(level)
            .map(|task| planned_instance_id(&self.run_id, task))
            .collect()
    }

    /// End every task instance this run had not finished at `at`: `StopWorkflowRun`
    /// "terminates all running tasks", and a task that was running or waiting when the
    /// shutdown reached it ran to neither success nor failure.
    ///
    /// Only the two statuses this emulator's own execution assigns are ended, so an
    /// instance that had already finished keeps the outcome it finished with. An
    /// instance whose attempt had STARTED owes the stream it was writing to the line
    /// saying how it ended; one that never started wrote nothing and names no stream.
    fn cancel_unfinished_tasks(&mut self, at: SystemTime) {
        let mut lines: Vec<(String, String)> = Vec::new();
        for instance in self.task_instances.values_mut() {
            if instance.status != TASK_INSTANCE_STATUS_RUNNING
                && instance.status != TASK_INSTANCE_STATUS_SCHEDULED
            {
                continue;
            }
            instance.status = TASK_INSTANCE_STATUS_CANCELLED.to_string();
            instance.ended_at = instance.started_at.map(|_| at);
            instance.modified_at = at;
            if let Some(stream) = instance.log_stream.clone() {
                lines.push((stream, task_finished_message(instance)));
            }
        }
        for (stream, message) in lines {
            self.queue_log_line(stream, at, message);
        }
    }

    /// Whether this run has finished: its status is one a run never leaves
    /// ([`RUN_TERMINAL_STATUSES`]), so no execution of it remains to be stopped.
    pub fn is_terminal(&self) -> bool {
        RUN_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Start the shutdown `StopWorkflowRun` describes: a run that has not finished
    /// moves to [`RUN_STATUS_STOPPING`], and its arrival in [`RUN_STATUS_STOPPED`]
    /// falls due [`RUN_STOPPED_AFTER`] later — stored, never slept on, and
    /// materialised by [`WorkflowRun::settle`] like every other owed change. Reports
    /// whether this call moved the run.
    ///
    /// Idempotent, because the operation is `@idempotent` and models no conflict for a
    /// run there is nothing to stop: a run already stopping keeps the deadline it has,
    /// and a run that already finished keeps the status, completion and reason it
    /// finished with, so a repeated stop REPORTS the run rather than restarting it.
    pub fn begin_stop(&mut self, now: SystemTime) -> bool {
        if self.is_terminal() || self.status == RUN_STATUS_STOPPING {
            return false;
        }
        self.status = RUN_STATUS_STOPPING.to_string();
        self.modified_at = now;
        self.next_status_at = now + RUN_STOPPED_AFTER;
        self.log_status_reached(now);
        true
    }
}

/// The `TaskInstanceId` the run `run_id` records its instance of the planned task `task`
/// under: the identifier minted for the FIRST attempt at that task in that run
/// ([`mint_task_instance_id`]).
///
/// The single place this service turns a planned task into the identifier of the instance
/// executing it, so the execution that records an instance, the walk that advances it and
/// every reader that reports it address the same record. A run executes a declared task
/// once, which is why the attempt it names is [`FIRST_ATTEMPT_NUMBER`].
fn planned_instance_id(run_id: &str, task: &workflow_execution::PlannedTask) -> String {
    mint_task_instance_id(run_id, &task.task_id, FIRST_ATTEMPT_NUMBER)
}

/// The line a task instance's own stream owes for the attempt that has just STARTED: the
/// status the instance entered, and the operator the definition declared for it when it
/// declared one this service can report.
///
/// The line states the TRANSITION that was materialised and nothing beyond it: no worker
/// ran the task and no operator was executed here (`workflow_execution`), so the text
/// claims neither.
fn task_started_message(instance: &TaskInstance) -> String {
    let task_id = instance
        .task_id
        .as_deref()
        .unwrap_or(&instance.task_instance_id);
    let attempt = instance.attempt_number.unwrap_or(FIRST_ATTEMPT_NUMBER);
    let status = &instance.status;
    match instance.operator_name.as_deref() {
        Some(operator) => format!(
            "Task {task_id} attempt {attempt} entered status {status}, declared operator \
             {operator}."
        ),
        None => format!("Task {task_id} attempt {attempt} entered status {status}."),
    }
}

/// The line a task instance's own stream owes for the attempt that has just ENDED: the
/// status it ended in, and the span it occupied for an attempt that had started.
fn task_finished_message(instance: &TaskInstance) -> String {
    let task_id = instance
        .task_id
        .as_deref()
        .unwrap_or(&instance.task_instance_id);
    let attempt = instance.attempt_number.unwrap_or(FIRST_ATTEMPT_NUMBER);
    let status = &instance.status;
    match instance.duration_in_seconds() {
        Some(seconds) => format!(
            "Task {task_id} attempt {attempt} entered status {status} after {seconds} seconds."
        ),
        None => format!("Task {task_id} attempt {attempt} entered status {status}."),
    }
}

/// The stream one task instance's attempt writes to, as the run that holds it names it
/// ([`run_logs::task_log_stream`]).
fn task_log_stream_of(run_id: &str, instance: &TaskInstance) -> String {
    run_logs::task_log_stream(
        run_id,
        &instance.task_instance_id,
        instance.attempt_number.unwrap_or(FIRST_ATTEMPT_NUMBER),
    )
}

/// One task instance of a run: the record of a single task of the workflow
/// definition, inside one run of it.
///
/// The fields are exactly what the modelled task-instance shapes report
/// (`smithy_query(query="shape", shape="GetTaskInstanceResponse,TaskInstanceSummary")`),
/// minus the three a reader takes from the run that owns it (`WorkflowArn`, `RunId`,
/// `WorkflowVersion`) and the `DurationInSeconds` a reader DERIVES from
/// `started_at`/`ended_at` ([`TaskInstance::duration_in_seconds`]).
///
/// Written by the run's own execution of the task graph its definition declared
/// ([`WorkflowRun::settle`]), so a run holds one of these per declared task.
#[derive(Debug, Clone)]
pub struct TaskInstance {
    /// `TaskInstanceId`: this instance's own identifier, the key it is stored under.
    pub task_instance_id: String,
    /// `TaskId`: the task in the workflow definition this instance executes.
    pub task_id: Option<String>,
    /// `Status`: a `TaskInstanceStatus` value (see [`TASK_INSTANCE_STATUS_NONE`]).
    pub status: String,
    /// `OperatorName`: the Apache Airflow operator the task uses.
    pub operator_name: Option<String>,
    /// `AttemptNumber`: which attempt this instance is.
    pub attempt_number: Option<i32>,
    /// `StartedAt`: `None` until the task starts, per the member's docs.
    pub started_at: Option<SystemTime>,
    /// `EndedAt`: `None` until the task completes, per the member's docs.
    pub ended_at: Option<SystemTime>,
    /// `ModifiedAt`: when this record last changed.
    pub modified_at: SystemTime,
    /// `ErrorMessage`: `None` when the task did not fail, per the member's docs.
    pub error_message: Option<String>,
    /// `LogStream`: the CloudWatch log stream this instance logs to.
    pub log_stream: Option<String>,
    /// `Xcom`: the cross-task data this instance exchanged. A `BTreeMap` so the
    /// entries have one order in the store.
    pub xcom: BTreeMap<String, String>,
}

impl TaskInstance {
    /// `DurationInSeconds`, derived rather than stored: the wall time between
    /// `started_at` and `ended_at`, and `None` while the task is not complete —
    /// "This value is null if the task is not complete"
    /// (`smithy_query(query="shape", shape="GetTaskInstanceResponse")`,
    /// `DurationInSeconds`).
    pub fn duration_in_seconds(&self) -> Option<i32> {
        let (started, ended) = (self.started_at?, self.ended_at?);
        Some(duration_in_seconds(
            ended.duration_since(started).unwrap_or_default(),
        ))
    }
}

/// Which attempt an instance is when it names no attempt number: a run executes a
/// declared task once, so the instances this service's own execution records are all the
/// first attempt at their task.
pub const FIRST_ATTEMPT_NUMBER: i32 = 1;

/// The `TaskInstanceStatus` values this service's execution assigns, as the model
/// spells them (`smithy_query(query="shape", shape="TaskInstanceStatus")`).
///
/// The value for an instance that has no status yet. Nothing in this emulator's own
/// execution assigns it — an instance is recorded when its run starts executing — so
/// it is the value a seeded record starts from in
/// `tests/all/task_instance_reads.rs`.
pub const TASK_INSTANCE_STATUS_NONE: &str = "NONE";

/// The status an instance of a task waiting for the tasks it depends on carries.
pub const TASK_INSTANCE_STATUS_SCHEDULED: &str = "SCHEDULED";

/// The status an instance carries while its task is being executed.
pub const TASK_INSTANCE_STATUS_RUNNING: &str = "RUNNING";

/// The status an instance of a task that finished successfully carries.
pub const TASK_INSTANCE_STATUS_SUCCESS: &str = "SUCCESS";

/// The status an instance that was still running or waiting when its run was stopped
/// carries: the stop "terminates all running tasks", so the task ran to neither
/// success nor failure.
pub const TASK_INSTANCE_STATUS_CANCELLED: &str = "CANCELLED";

/// The status an instance of a task whose own work answered FAILURE carries
/// (`GetTaskInstanceResponse$ErrorMessage` is "The error message if the task instance
/// failed", so an instance that failed is one the model reports a reason for).
pub const TASK_INSTANCE_STATUS_FAILED: &str = "FAILED";

/// The status an instance of a task that never ran because a task it waits for failed
/// carries: the work it depends on did not finish, so this task was not executed at all.
pub const TASK_INSTANCE_STATUS_UPSTREAM_FAILED: &str = "UPSTREAM_FAILED";

/// The instant `span` after `at`, and `at` itself for a sum no clock can hold.
///
/// Why checked: part of a run's schedule is the time the work its definition declares
/// states it takes ([`workflow_execution::ExecutionPlan::level_runs_for`]), which
/// originates in caller-supplied definition content. That span is bounded where it is read
/// ([`declared_runtime::MAX_DECLARED_RUNTIME`]), so the fallback is not reachable through
/// any definition; it is here because every transition is materialised while this
/// service's shared store is held ([`WorkflowRun::settle`]), where a panicking addition
/// would be a failure for every later caller rather than for one call. A deadline past the
/// end of time is one nothing can still be waiting for, so the change falls due at once.
fn deadline_after(at: SystemTime, span: Duration) -> SystemTime {
    at.checked_add(span).unwrap_or(at)
}

/// A run duration in whole seconds, rounded up so a sub-second run still reports
/// the second it occupied, and clamped into the modelled `Integer`.
fn duration_in_seconds(elapsed: Duration) -> i32 {
    let secs = elapsed.as_millis().div_ceil(1000);
    i32::try_from(secs).unwrap_or(i32::MAX)
}

/// A persisted workflow.
#[derive(Debug, Clone)]
pub struct Workflow {
    pub arn: String,
    /// The region whose endpoint this workflow was created through: the credential
    /// scope of the `CreateWorkflow` request ([`request_region`]), which is also the
    /// region segment of [`Workflow::arn`].
    ///
    /// A regional endpoint serves only the resources created through it, so this is
    /// the partition key EVERY lookup, listing and name-uniqueness check of this
    /// store is scoped by ([`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::workflow_in`]).
    pub region: String,
    pub name: String,
    pub status: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub current_version: String,
    pub revision_id: String,
    pub versions: Vec<WorkflowVersionRecord>,
    /// The tag set this workflow carries, keyed by tag key: at most
    /// [`MAX_TAGS_PER_RESOURCE`] entries, which is the size the modelled `Tags` map can
    /// report.
    pub tags: BTreeMap<String, String>,
    pub runs: BTreeMap<String, WorkflowRun>,
    /// The instant the clock owes this workflow's removal, once its teardown has
    /// been started by `DeleteWorkflow`, and `None` for a workflow no delete has
    /// touched. Stored, never slept on, and materialised by
    /// [`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::reap_deleted_workflows`].
    pub deletion_due_at: Option<SystemTime>,
    /// The instant the clock owes this workflow's NEXT automated run, for a workflow
    /// whose active version's definition declares a schedule
    /// ([`Workflow::is_scheduled`]), and `None` for one only a caller starts or whose
    /// teardown has removed its triggers. Stored, never slept on, and materialised by
    /// [`ComAmazonawsMwaaserverlessAmazonmwaaserverlessState::fire_due_scheduled_runs`].
    pub next_scheduled_run_at: Option<SystemTime>,
}

impl Workflow {
    /// The version record for `version`, or the latest one when `version` is
    /// `None`.
    pub fn version(&self, version: Option<&str>) -> Option<&WorkflowVersionRecord> {
        match version {
            Some(v) => self.versions.iter().find(|rec| rec.version == v),
            None => self.versions.iter().find(|rec| rec.is_latest),
        }
    }

    /// The configuration this workflow currently reports.
    pub fn current_snapshot(&self) -> Option<&WorkflowSnapshot> {
        self.version(None).map(|rec| &rec.snapshot)
    }

    /// Whether this workflow's teardown has already been started, so it is on its
    /// way out and no longer accepts a change.
    pub fn is_deleting(&self) -> bool {
        self.deletion_due_at.is_some()
    }

    /// Whether the version this workflow currently runs carries the scheduling
    /// information the service configures an automated trigger from: the schedule its
    /// snapshotted definition declares
    /// ([`WorkflowSnapshot::schedule_configuration`]).
    ///
    /// The definition is where that information lives — "when a workflow includes
    /// schedule information in its YAML definition, the service automatically
    /// configures the appropriate triggers for automated execution"
    /// (`smithy_query(query="shape", shape="GetWorkflowResponse")`,
    /// `ScheduleConfiguration`) — so this reads the one input the service's own
    /// scheduling is derived from, the same one `ScheduleConfiguration` reports. A
    /// version whose definition declares no schedule the service can configure a
    /// trigger from, and one holding no snapshotted definition at all, is left to be
    /// started by a caller; no request member stands in for the declaration.
    pub fn is_scheduled(&self) -> bool {
        self.current_snapshot()
            .is_some_and(|snapshot| snapshot.schedule_configuration.is_some())
    }

    /// Point this workflow's automated trigger at the definition its ACTIVE version
    /// froze: a version whose definition declares a schedule ([`Self::is_scheduled`])
    /// owes its first firing [`SCHEDULED_RUN_EVERY`] from `now`, and one that declares
    /// none clears the trigger.
    ///
    /// Called wherever the active version changes, because "only one version of a
    /// workflow can have an active schedule at any given time"
    /// (`smithy_query(query="shape", shape="GetWorkflowResponse")`,
    /// `ScheduleConfiguration`): the trigger in force is always the latest version's,
    /// so a workflow re-pointed at a definition declaring no schedule stops producing
    /// runs of its own.
    pub fn configure_trigger(&mut self, now: SystemTime) {
        self.next_scheduled_run_at = self.is_scheduled().then(|| now + SCHEDULED_RUN_EVERY);
    }

    /// Make the newest APPENDED version of this workflow its ONLY latest one, and
    /// re-derive from that version everything the workflow reports as its current
    /// configuration.
    ///
    /// Exactly one version is the latest at any instant: the flag is cleared on every
    /// record and set on the one record holding the greatest
    /// [`WorkflowVersionRecord::sequence`] — the newest APPENDED version, which is
    /// also the last of the append order a version listing walks
    /// ([`format_version_listing_key`]). The append position, not the wall clock,
    /// decides, because two records can carry the same `created_at` instant while
    /// their sequence is unique and only ever grows.
    ///
    /// Called wherever the set of versions changes the active one — the first
    /// version appended by a create, a further version appended by an update, or
    /// the latest version removed by a delete — so
    /// [`Workflow::current_version`], [`Workflow::revision_id`] and the automated
    /// trigger ([`Self::configure_trigger`]) always describe the one version
    /// [`Self::version`] resolves with no version named. A workflow holding no
    /// version left has nothing to promote, and keeps what it last reported.
    pub fn promote_newest_version(&mut self, now: SystemTime) {
        let newest = self
            .versions
            .iter()
            .enumerate()
            .max_by_key(|(_, record)| record.sequence)
            .map(|(index, _)| index);
        for record in &mut self.versions {
            record.is_latest = false;
        }
        let Some(newest) = newest else {
            return;
        };
        self.versions[newest].is_latest = true;
        self.current_version = self.versions[newest].version.clone();
        self.revision_id = mint_revision_id(&self.arn, &self.current_version);
        self.configure_trigger(now);
    }

    /// Start the teardown `DeleteWorkflow` describes: the workflow moves to
    /// [`WORKFLOW_STATUS_DELETING`], every execution of it that has not finished is
    /// stopped ([`Self::stop_runs`]) and its automated trigger stops firing, because
    /// the cleanup the operation performs includes "stopping any running executions,
    /// removing scheduled triggers", and its removal falls due
    /// [`WORKFLOW_DELETED_AFTER`] later. Nothing is DROPPED here — the versions,
    /// runs, name and tokens go with the record when the removal lands, so a read
    /// during the teardown still reports the workflow and the runs it stopped.
    ///
    /// Idempotent: a second delete of a workflow already tearing down neither
    /// restarts the teardown nor moves its deadline, and leaves the runs the first
    /// one stopped on the shutdown schedule they already carry.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.status = WORKFLOW_STATUS_DELETING.to_string();
        self.modified_at = now;
        self.deletion_due_at = Some(now + WORKFLOW_DELETED_AFTER);
        // "removing scheduled triggers": the workflow's own trigger fires no more, so
        // the teardown is not racing new automated runs into a record on its way out.
        self.next_scheduled_run_at = None;
        // The workflow is going, so no execution of it may keep progressing: each
        // unfinished run enters the documented shutdown instead.
        self.stop_runs(None, now);
    }

    /// Stop every execution the removal of `version` takes with it — or EVERY
    /// execution of this workflow when `version` is `None`, which is the removal of
    /// the workflow itself — and report how many runs this call moved.
    ///
    /// Removing a workflow definition stops the executions of it that are still in
    /// flight: each such run enters the same shutdown [`WorkflowRun::begin_stop`]
    /// drives for `StopWorkflowRun` ([`RUN_STATUS_STOPPING`], then
    /// [`RUN_STATUS_STOPPED`] when [`RUN_STOPPED_AFTER`] is up), so nothing is left
    /// being admitted to, or failed by, a definition that is on its way out.
    ///
    /// A run that already finished keeps the status, completion and reason it
    /// finished with: the changes the clock already owed are applied first, so a run
    /// is stopped out of the state it really was in at `now` rather than out of the
    /// one it was last written in.
    pub fn stop_runs(&mut self, version: Option<&str>, now: SystemTime) -> usize {
        let mut stopped = 0;
        for run in self.runs.values_mut() {
            if version.is_some_and(|wanted| run.workflow_version != wanted) {
                continue;
            }
            run.settle(now);
            if run.begin_stop(now) {
                stopped += 1;
            }
        }
        stopped
    }

    /// Take the lines every run of this workflow owes its log group, leaving none
    /// behind: what a handler that moved a whole workflow's runs — a teardown stopping
    /// the executions it takes with it ([`Self::stop_runs`]) — delivers once it has
    /// released this store ([`run_logs::publish`]).
    pub fn take_pending_log_lines(&mut self) -> Vec<run_logs::LogLine> {
        self.runs
            .values_mut()
            .flat_map(WorkflowRun::take_pending_log_lines)
            .collect()
    }
}

/// What a `CreateWorkflow` call ANSWERED for the workflow it created, beside the
/// workflow's identity: the version that call created and everything the call reported
/// about it.
///
/// Kept in the ledger because none of it can be re-derived from the workflow later. A
/// created workflow owes the clock no transition of its own — it is returned
/// [`WORKFLOW_STATUS_READY`] — so every one of these values changes only when ANOTHER
/// caller acts: appending a version moves [`Workflow::current_version`],
/// [`Workflow::revision_id`] and the latest-version flag
/// ([`Workflow::promote_newest_version`]), and starting a teardown moves
/// [`Workflow::status`]. Recomputing them at replay time would therefore answer values
/// the first use never produced, so the first use's own values are recorded here.
#[derive(Debug, Clone)]
pub struct CreatedWorkflow {
    /// `CreatedAt`: when the workflow was created.
    pub created_at: SystemTime,
    /// `WorkflowVersion`: the version the create itself created.
    pub workflow_version: String,
    /// `RevisionId`: the revision the create reported for that version.
    pub revision_id: String,
    /// `WorkflowStatus`: the status the create reported for the workflow.
    pub workflow_status: String,
    /// `IsLatestVersion`: whether the created version was the workflow's latest one.
    pub is_latest_version: bool,
}

impl CreatedWorkflow {
    /// What a create answers for the workflow record it just stored: the active
    /// version, its revision, the workflow's status and whether that version is the
    /// latest — every value read out of the record, never computed beside it.
    pub fn of(workflow: &Workflow) -> Self {
        Self {
            created_at: workflow.created_at,
            workflow_version: workflow.current_version.clone(),
            revision_id: workflow.revision_id.clone(),
            workflow_status: workflow.status.clone(),
            is_latest_version: workflow
                .version(Some(&workflow.current_version))
                .is_some_and(|record| record.is_latest),
        }
    }
}

/// What a `ClientToken` already produced, so repeating it replays rather than
/// creating a second resource.
///
/// A variant keeps whatever its operation's replay cannot read back out of the live
/// record: the workflow a create created ANSWERED for ([`CreatedWorkflow`]), because a
/// later call can move every value of that answer, while a start keeps only the run's
/// identity, because a run's own answer moves with nothing but the clock that was
/// already running when it was started ([`WorkflowRun::settle`]).
#[derive(Debug, Clone)]
pub enum IdempotentResult {
    CreateWorkflow {
        request_fingerprint: String,
        workflow_arn: String,
        answered: CreatedWorkflow,
    },
    StartWorkflowRun {
        request_fingerprint: String,
        workflow_arn: String,
        run_id: String,
    },
}

impl IdempotentResult {
    /// The fingerprint of the request that first used the token.
    pub fn request_fingerprint(&self) -> &str {
        match self {
            Self::CreateWorkflow {
                request_fingerprint,
                ..
            }
            | Self::StartWorkflowRun {
                request_fingerprint,
                ..
            } => request_fingerprint,
        }
    }

    /// The workflow the token's first use touched.
    pub fn workflow_arn(&self) -> &str {
        match self {
            Self::CreateWorkflow { workflow_arn, .. }
            | Self::StartWorkflowRun { workflow_arn, .. } => workflow_arn,
        }
    }
}

/// A bounded `<operation>:<ClientToken>` -> first-result ledger.
///
/// Insertion-ordered and capped at [`MAX_IDEMPOTENCY_TOKENS`]: recording the (cap + 1)-th
/// token forgets the oldest one rather than growing the map.
#[derive(Debug, Default)]
pub struct IdempotencyLedger {
    by_key: BTreeMap<String, IdempotentResult>,
    oldest_first: VecDeque<String>,
}

impl IdempotencyLedger {
    /// What a previous call recorded under `key`, if it is still remembered.
    pub fn get(&self, key: &str) -> Option<&IdempotentResult> {
        self.by_key.get(key)
    }

    /// Remember `result` under `key`, evicting the oldest entries to stay within
    /// [`MAX_IDEMPOTENCY_TOKENS`]. Re-recording a key moves it to the back, so the
    /// eviction order stays insertion-ordered by the LAST time a key was written.
    pub fn record(&mut self, key: String, result: IdempotentResult) {
        if self.by_key.insert(key.clone(), result).is_some() {
            self.oldest_first.retain(|recorded| recorded != &key);
        }
        self.oldest_first.push_back(key);
        while self.oldest_first.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many tokens are remembered right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether no token is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Forget every token that names `workflow_arn` — called when that workflow is
    /// deleted, so a token record never outlives the resource it created.
    pub fn forget_workflow(&mut self, workflow_arn: &str) {
        self.by_key
            .retain(|_, result| result.workflow_arn() != workflow_arn);
        let live = &self.by_key;
        self.oldest_first.retain(|key| live.contains_key(key));
    }
}

/// The `@idempotencyToken` ledger of ONE operation at ONE regional endpoint, and the
/// whole rule a handler follows with the `ClientToken` a request carries.
///
/// Every operation of this service that admits a token takes the same four steps with
/// it — build the per-operation, per-region ledger key, look the token up, refuse a
/// first use whose request digests differently, otherwise answer from the record that
/// first use left — so the four live HERE, once, parameterised by the only two things
/// an operation differs in: the operation the token was presented to and the
/// `ResourceType` its refusal reports. A change to the key shape, to the comparison or
/// to the identity a conflict reports therefore reaches every such operation at once.
///
/// The handler keeps only what is its own: the record it writes ([`IdempotentResult`])
/// and how it answers from a replayed one.
#[derive(Debug, Clone, Copy)]
pub struct TokenLedger<'a> {
    /// The operation the token was presented to. It leads the ledger key — so one
    /// token used on two operations records two entries — and names the operation a
    /// mismatch refusal reports.
    operation: &'static str,
    /// The `ResourceType` a mismatch refusal reports: the kind of resource this
    /// operation's first use of a token produced.
    resource_type: &'static str,
    /// The endpoint the token was presented at, because a token presented at one
    /// regional endpoint says nothing about what another endpoint produced.
    region: &'a str,
}

impl<'a> TokenLedger<'a> {
    /// The token ledger of `operation` at `region`'s endpoint, whose mismatch refusals
    /// report `resource_type`.
    pub const fn of(operation: &'static str, resource_type: &'static str, region: &'a str) -> Self {
        Self {
            operation,
            resource_type,
            region,
        }
    }

    /// The key a `ClientToken` of this operation and this endpoint is remembered
    /// under, so one token used on two different operations — or at two different
    /// regional endpoints — records two entries.
    pub fn key(&self, client_token: &str) -> String {
        format!("{}:{}:{}", self.operation, self.region, client_token)
    }

    /// What the token this request carries already produced: `Some(record)` to replay
    /// that first answer from, or `None` when the request carries no token or carries
    /// one nothing is remembered under, so the call does its work.
    ///
    /// A token remembered for a request that digests differently is the modelled
    /// `ConflictException` instead, naming the token, the operation it was first used
    /// for and the identity of the resource that first use produced; the handler wraps
    /// it in its own error variant, which is the one thing it cannot share.
    pub fn replay(
        &self,
        ledger: &IdempotencyLedger,
        client_token: Option<&str>,
        request_fingerprint: &str,
    ) -> Result<Option<IdempotentResult>, ConflictException> {
        let Some(token) = client_token else {
            return Ok(None);
        };
        let Some(recorded) = ledger.get(&self.key(token)) else {
            return Ok(None);
        };
        if recorded.request_fingerprint() != request_fingerprint {
            return Err(ConflictException {
                message: format!(
                    "ClientToken {token} was already used for a {} request with different \
                     parameters",
                    self.operation
                ),
                resource_id: recorded.workflow_arn().to_string(),
                resource_type: self.resource_type.to_string(),
            });
        }
        Ok(Some(recorded.clone()))
    }

    /// Remember `result` under the token this request carries, so repeating the token
    /// replays that answer through [`Self::replay`]. A request carrying no token
    /// records nothing: there is nothing for a later call to present.
    pub fn record(
        &self,
        ledger: &mut IdempotencyLedger,
        client_token: Option<&str>,
        result: IdempotentResult,
    ) {
        if let Some(token) = client_token {
            ledger.record(self.key(token), result);
        }
    }
}

/// Per-service state for com-amazonaws-mwaaserverless-amazonmwaaserverless. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMwaaserverlessAmazonmwaaserverlessState {
    /// Every persisted workflow, keyed by its ARN. A `BTreeMap` so `ListWorkflows`
    /// has one total order to paginate over. At most [`MAX_WORKFLOWS`] entries, and at
    /// most [`MAX_RUNS_PER_WORKFLOW`] runs inside each.
    ///
    /// A `WorkflowArn` embeds the region it was minted for, so one map holds every
    /// region's workflows without collision; a handler reaches only the ones the
    /// caller's endpoint serves, through [`Self::workflow_in`],
    /// [`Self::workflow_in_mut`] and [`Self::workflows_in_after`].
    pub workflows: BTreeMap<String, Workflow>,
    /// `(region, Name)` -> `WorkflowArn`, the uniqueness index `CreateWorkflow`
    /// conflicts on. The region is part of the key because a name is taken only at the
    /// endpoint the workflow was created through, so the same name is free in every
    /// other region. One key per stored workflow — including one that is `DELETING`,
    /// whose name stays taken until its teardown completes — so [`MAX_WORKFLOWS`]
    /// bounds this map too.
    pub workflow_names: BTreeMap<(String, String), String>,
    /// `<operation>:<ClientToken>` -> what that token already produced. Capped and
    /// evicted on its own terms; see [`IdempotencyLedger`].
    pub idempotency: IdempotencyLedger,
    /// Monotonic counter mixed into every minted identifier.
    pub mint_seq: u64,
}

impl ComAmazonawsMwaaserverlessAmazonmwaaserverlessState {
    /// A fresh nonce for an identifier mint: a monotonic counter plus the wall
    /// clock, so two mints never collide even within one nanosecond.
    pub fn next_nonce(&mut self, now: SystemTime) -> String {
        self.mint_seq += 1;
        let nanos = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}:{nanos}", self.mint_seq)
    }

    /// Forget every idempotency token whose resource is `workflow_arn`, so a
    /// replayed token can never resolve to a workflow that no longer exists.
    pub fn forget_tokens_for(&mut self, workflow_arn: &str) {
        self.idempotency.forget_workflow(workflow_arn);
    }

    /// Finish every workflow teardown the clock has already made due, and report
    /// whether anything was removed.
    ///
    /// This is the one write a reporting operation may make: the removal of a
    /// workflow whose [`Workflow::deletion_due_at`] has passed was owed at that
    /// instant whether or not anybody called, so what a reader reports is what the
    /// store would hold unpolled. Removing the record releases the name
    /// `CreateWorkflow` conflicts on and forgets the tokens that replayed onto it,
    /// and takes the workflow's versions and runs with it.
    ///
    /// Every handler that touches [`Self::workflows`] calls this first, so a
    /// workflow never outlives its teardown just because the next request happened
    /// to be a different operation.
    pub fn reap_deleted_workflows(&mut self, now: SystemTime) -> bool {
        let due: Vec<(String, String, String)> = self
            .workflows
            .values()
            .filter(|workflow| workflow.deletion_due_at.is_some_and(|at| now >= at))
            .map(|workflow| {
                (
                    workflow.arn.clone(),
                    workflow.region.clone(),
                    workflow.name.clone(),
                )
            })
            .collect();
        for (arn, region, name) in &due {
            self.workflows.remove(arn);
            // Only if the name still points at THIS workflow: the index is keyed by
            // region and name, and a workflow removed earlier may have freed it already.
            let name_key = (region.clone(), name.clone());
            if self.workflow_names.get(&name_key).is_some_and(|held| held == arn) {
                self.workflow_names.remove(&name_key);
            }
            self.forget_tokens_for(arn);
        }
        !due.is_empty()
    }

    /// Materialise every automated run this store's triggers already owed, and report
    /// how many runs that created.
    ///
    /// A workflow whose active version's definition declares a schedule carries the
    /// instant its trigger next fires ([`Workflow::next_scheduled_run_at`]). A firing
    /// at or before `now` was owed at THAT instant whether or not anybody called, so
    /// the run it produces is recorded with the due instant as its creation and start
    /// and the trigger moves on to its next firing: this is the one write a reporting
    /// operation may make, and it is what makes two reads after the same firing answer
    /// identically. The run then walks the same lifecycle a caller-started one does
    /// ([`WorkflowRun::settle`]), and reports [`RUN_TYPE_SCHEDULED`] because the
    /// workflow's own trigger started it rather than a caller.
    ///
    /// A workflow already holding [`MAX_RUNS_PER_WORKFLOW`] runs records no further
    /// one: its trigger skips to the next firing after `now`, so a schedule left
    /// running cannot grow this store without bound.
    ///
    /// One call materialises at most [`MAX_CATCHUP_FIRINGS_PER_WORKFLOW`] firings for a
    /// workflow and [`MAX_CATCHUP_FIRINGS_PER_CALL`] in all, so the work one request
    /// performs does not grow with the gap it found. A trigger a capped call stopped at
    /// is left THERE rather than moved on, so nothing is skipped and the next call
    /// continues the catch-up — and the triggers are served OLDEST OWED FIRING FIRST,
    /// so the trigger a capped call did not reach holds the oldest firing in the store
    /// and is the first the next call serves. No trigger can therefore be starved by
    /// one that keeps falling due.
    pub fn fire_due_scheduled_runs(&mut self, now: SystemTime) -> usize {
        let mut due: Vec<(SystemTime, String)> = self
            .workflows
            .values()
            .filter_map(|workflow| {
                workflow
                    .next_scheduled_run_at
                    .filter(|at| now >= *at)
                    .map(|at| (at, workflow.arn.clone()))
            })
            .collect();
        due.sort();
        let mut fired = 0;
        for (_, arn) in due {
            if fired >= MAX_CATCHUP_FIRINGS_PER_CALL {
                break;
            }
            // The version in force and the plan a run of it follows are resolved ONCE
            // for this workflow's whole catch-up: every firing of it executes the same
            // frozen definition, so the definition content is neither cloned nor read
            // again per firing. The log group that version configures is resolved with
            // them, because it is where the runs these firings produce export their
            // output.
            let Some((version, execution, log_group_name)) = self.active_execution(&arn) else {
                continue;
            };
            let mut caught_up = 0;
            // Every firing the clock owes, oldest first, so a trigger left behind
            // reports one run per firing rather than one run for the whole gap.
            while let Some((due_at, at_capacity)) = self.next_owed_firing(&arn, now) {
                if at_capacity {
                    let skipped = firing_after(due_at, now, SCHEDULED_RUN_EVERY);
                    if let Some(workflow) = self.workflows.get_mut(&arn) {
                        workflow.next_scheduled_run_at = Some(skipped);
                    }
                    break;
                }
                if caught_up >= MAX_CATCHUP_FIRINGS_PER_WORKFLOW
                    || fired >= MAX_CATCHUP_FIRINGS_PER_CALL
                {
                    break;
                }

                let nonce = self.next_nonce(due_at);
                let sequence = self.mint_seq;
                let run_id = mint_run_id(&arn, &nonce);
                let mut run = WorkflowRun {
                    run_id: run_id.clone(),
                    workflow_arn: arn.clone(),
                    workflow_version: version.clone(),
                    // Nobody asked for this run: the workflow's own trigger did.
                    run_type: RUN_TYPE_SCHEDULED.to_string(),
                    status: RUN_STATUS_STARTING.to_string(),
                    // A trigger carries no caller parameters to override with, so the
                    // run executes with the one parameter the service injects itself.
                    override_parameters: run_parameters(&arn, []),
                    // Dated from the DUE instant, never from `now`, so the run a
                    // reader reports is the one the firing would have produced
                    // unpolled.
                    created_at: due_at,
                    started_on: due_at,
                    completed_on: None,
                    modified_at: due_at,
                    duration: None,
                    error_message: None,
                    // The instances of the tasks this run's definition declares are
                    // recorded when it starts executing them ([`WorkflowRun::settle`]).
                    task_instances: BTreeMap::new(),
                    execution: execution.clone(),
                    next_status_at: due_at + RUN_ADMITTED_AFTER,
                    log_group_name: log_group_name.clone(),
                    pending_log_lines: Vec::new(),
                    sequence,
                };
                // The first line this run's output owes: it was admitted, by the
                // workflow's own trigger, at the instant the firing fell due.
                run.log_admitted();

                let Some(workflow) = self.workflows.get_mut(&arn) else {
                    break;
                };
                workflow.runs.insert(run_id, run);
                workflow.next_scheduled_run_at = Some(due_at + SCHEDULED_RUN_EVERY);
                caught_up += 1;
                fired += 1;
            }
        }
        fired
    }

    /// The version in force for `arn`, the execution plan a run of that version
    /// follows, built once from the definition content the version froze, and the
    /// CloudWatch Logs log group that version's `LoggingConfiguration` names — where a
    /// run of it exports its output ([`run_logs`]).
    fn active_execution(
        &self,
        arn: &str,
    ) -> Option<(
        String,
        workflow_execution::ExecutionPlan,
        Option<String>,
    )> {
        let workflow = self.workflows.get(arn)?;
        let version = workflow.version(None)?;
        Some((
            version.version.clone(),
            workflow_execution::ExecutionPlan::of(
                version.snapshot.workflow_definition.as_deref(),
            ),
            configured_log_group(&version.snapshot),
        ))
    }

    /// The next firing of `arn`'s trigger that `now` has already made due: the instant
    /// it fell due, and whether the workflow already holds every run this store keeps
    /// for it.
    ///
    /// `None` when the trigger owes nothing: the workflow is gone, it has no trigger,
    /// its next firing is still ahead, or it holds no version to run.
    fn next_owed_firing(&self, arn: &str, now: SystemTime) -> Option<(SystemTime, bool)> {
        let workflow = self.workflows.get(arn)?;
        let due_at = workflow.next_scheduled_run_at.filter(|at| now >= *at)?;
        // A workflow holding no version has nothing for a firing to execute.
        workflow.version(None)?;
        Some((due_at, workflow.runs.len() >= MAX_RUNS_PER_WORKFLOW))
    }

    /// Apply EVERY change the clock already owed this store — the workflow teardowns
    /// whose removal has fallen due ([`Self::reap_deleted_workflows`]) and the
    /// automated runs whose firing has ([`Self::fire_due_scheduled_runs`]) — and answer
    /// the instant that was done AT.
    ///
    /// Every handler that touches [`Self::workflows`] calls this FIRST, so what it
    /// answers is what the store would hold unpolled: a workflow never outlives its
    /// teardown, and a trigger never fails to fire, just because the next request
    /// happened to be a different operation.
    ///
    /// The instant is sampled HERE, with this store already locked, and is also the
    /// instant the calling handler stamps its own writes with — this is the ONLY place
    /// this service reads the wall clock while serving a request. A handler that sampled
    /// it earlier would materialise an instant that is already past by the time it
    /// reaches the store, so anything that fell due in between (a teardown that frees a
    /// workflow's name, a trigger firing) would stay unapplied under a record the
    /// handler then reads, and a timestamp it stamps could land before one a writer that
    /// held the store in between had already stamped. Sampling under the lock is what
    /// keeps both true however long the handler took to get here — including across an
    /// awaited call into another emulated service.
    pub fn advance_clock(&mut self) -> SystemTime {
        let now = SystemTime::now();
        self.reap_deleted_workflows(now);
        self.fire_due_scheduled_runs(now);
        now
    }

    /// The workflow `arn` names AS SEEN FROM `region`'s endpoint: the stored record
    /// when it lives in that region, and `None` otherwise.
    ///
    /// An AWS regional endpoint resolves only the resources created through it, so a
    /// reference to a workflow of another region is unresolvable here — the same miss
    /// as an ARN the store never held.
    pub fn workflow_in(&self, region: &str, arn: &str) -> Option<&Workflow> {
        self.workflows
            .get(arn)
            .filter(|workflow| workflow.region == region)
    }

    /// [`Self::workflow_in`], for a caller that has to write the record it resolves.
    pub fn workflow_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut Workflow> {
        self.workflows
            .get_mut(arn)
            .filter(|workflow| workflow.region == region)
    }

    /// The workflows `region`'s endpoint holds, in ARN order, starting after the ARN
    /// `after` names — the walk a region-scoped listing pages over.
    pub fn workflows_in_after<'a>(
        &'a self,
        region: &'a str,
        after: Option<&'a str>,
    ) -> impl Iterator<Item = &'a Workflow> + 'a {
        let lower = match after {
            Some(last) => std::ops::Bound::Excluded(last),
            None => std::ops::Bound::Unbounded,
        };
        self.workflows
            .range::<str, _>((lower, std::ops::Bound::Unbounded))
            .map(|(_, workflow)| workflow)
            .filter(move |workflow| workflow.region == region)
    }

    /// The ARN of the workflow holding `name` at `region`'s endpoint, if one does.
    /// A name taken in one region says nothing about any other.
    pub fn workflow_arn_named(&self, region: &str, name: &str) -> Option<&String> {
        self.workflow_names
            .get(&(region.to_string(), name.to_string()))
    }

    /// Record that `arn` holds `name` at `region`'s endpoint.
    pub fn claim_workflow_name(&mut self, region: &str, name: &str, arn: &str) {
        self.workflow_names
            .insert((region.to_string(), name.to_string()), arn.to_string());
    }
}

/// The first firing of a trigger of period `every` that falls strictly after `now`,
/// counted from the firing at `from`.
///
/// Computed rather than stepped, so a trigger whose store is full skips the whole gap
/// in one step instead of walking a firing at a time.
fn firing_after(from: SystemTime, now: SystemTime, every: Duration) -> SystemTime {
    let behind = now.duration_since(from).unwrap_or_default();
    let period = every.as_nanos().max(1);
    let skipped = u32::try_from(behind.as_nanos() / period + 1).unwrap_or(u32::MAX);
    from.checked_add(every.saturating_mul(skipped))
        // A gap no instant can represent: the trigger owes nothing further.
        .unwrap_or(now)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMwaaserverlessAmazonmwaaserverlessState>)
}

/// The region this request is scoped to: the SigV4 credential scope's region when it
/// is one the modelled `WorkflowArn` pattern accepts, else [`DEFAULT_REGION`].
///
/// It is both the region segment of an ARN minted for the request and the partition
/// every lookup, listing and name-uniqueness check is scoped by, because a regional
/// endpoint serves only the resources created through it.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// True when `region` matches `([a-z]{2}-[a-z]+-[0-9]{1})`, the region segment of
/// the modelled `WorkflowArn` pattern (`smithy_query shape=WorkflowArn`).
fn is_arn_region(region: &str) -> bool {
    let mut parts = region.split('-');
    let (Some(area), Some(direction), Some(index), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    area.len() == 2
        && area.bytes().all(|b| b.is_ascii_lowercase())
        && !direction.is_empty()
        && direction.bytes().all(|b| b.is_ascii_lowercase())
        && index.len() == 1
        && index.bytes().all(|b| b.is_ascii_digit())
}

/// A lowercase-hex SHA-256 digest over `fields`, each field length-delimited so
/// `["a", "bc"]` and `["ab", "c"]` never collide.
pub fn digest(fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// A field sequence being built for [`digest`], in a form that DETERMINES the
/// request it was built from: a member whose field count depends on what the
/// request carries announces how many fields it contributes before contributing
/// them, so the sequence can be read back as the member tree it came from.
///
/// This is what makes a digest usable to tell one request from another. A bare
/// field list is ambiguous as soon as one member's field count depends on the
/// request — an optional structure that contributes nothing when absent, or a
/// list whose elements are appended one field each — because then a value
/// supplied for one member lands in the position another member's value would
/// have taken, and the two requests digest alike.
#[derive(Debug, Default)]
pub struct DigestFields {
    fields: Vec<String>,
}

impl DigestFields {
    /// An empty sequence.
    pub fn new() -> Self {
        Self::default()
    }

    /// One field at a fixed position: a member every request of this kind
    /// carries, so its position never moves.
    pub fn field(&mut self, value: impl Into<String>) -> &mut Self {
        self.fields.push(value.into());
        self
    }

    /// A member whose extent depends on the request: the NUMBER of fields that
    /// follow, then those fields. The count is what makes the member
    /// self-delimiting, and a group nested inside a group keeps the whole
    /// sequence so — an absent member is simply the empty group.
    pub fn group(&mut self, build: impl FnOnce(&mut Self)) -> &mut Self {
        let mut group = Self::new();
        build(&mut group);
        self.fields.push(group.fields.len().to_string());
        self.fields.append(&mut group.fields);
        self
    }

    /// An optional scalar: the empty group when the request supplies none, a
    /// one-field group when it does — so a member the request omits and one it
    /// supplies as the empty string never digest alike.
    pub fn optional(&mut self, value: Option<&str>) -> &mut Self {
        self.group(|group| {
            if let Some(value) = value {
                group.field(value);
            }
        })
    }

    /// A list, as one group holding one field per element, so both its length
    /// and the order of its elements are part of the sequence.
    pub fn list<'a>(&mut self, values: impl IntoIterator<Item = &'a str>) -> &mut Self {
        self.group(|group| {
            for value in values {
                group.field(value);
            }
        })
    }

    /// An optional list: the empty group when the request supplies none, and the
    /// list's own group inside a group when it does, so an absent list and an
    /// empty one stay apart.
    pub fn optional_list<'a, I: IntoIterator<Item = &'a str>>(
        &mut self,
        values: Option<I>,
    ) -> &mut Self {
        self.group(|group| {
            if let Some(values) = values {
                group.list(values);
            }
        })
    }

    /// A caller-supplied `document` member, as ONE group whose fields are determined
    /// by the document's VALUE and by nothing else.
    ///
    /// A document's own `Debug` rendering is NOT usable here: an object inside it is a
    /// [`std::collections::HashMap`], which iterates in the order its own instance
    /// chose, so two deserializations of the same bytes render differently and the
    /// same request would digest two ways. The walk below therefore emits an object's
    /// members in sorted key order, and leads every node with its KIND and every
    /// collection with its SIZE, so one node's fields can never be read as another's:
    /// two documents digest alike exactly when they are the same document.
    ///
    /// Iterative, over an explicit worklist, so the walk costs heap rather than stack
    /// however deeply the caller nested the document.
    pub fn document(&mut self, value: &::aws_smithy_types::Document) -> &mut Self {
        self.group(|group| append_document(group, value))
    }

    /// The digest of the sequence built so far.
    pub fn digest(&self) -> String {
        let borrowed: Vec<&str> = self.fields.iter().map(String::as_str).collect();
        digest(&borrowed)
    }
}

/// [`DigestFields::document`]'s walk: append `value`'s canonical field sequence to
/// `fields`.
fn append_document(fields: &mut DigestFields, value: &::aws_smithy_types::Document) {
    use ::aws_smithy_types::{Document, Number};

    /// One entry of the worklist: a node still to be walked, or an object member's
    /// key, which is appended immediately before that member's value.
    enum Step<'a> {
        Node(&'a Document),
        Key(&'a str),
    }

    let mut pending = vec![Step::Node(value)];
    while let Some(step) = pending.pop() {
        match step {
            Step::Key(key) => {
                fields.field(key);
            }
            // Every node leads with the kind of value it is, so a string carrying a
            // number's text and that number never digest alike.
            Step::Node(Document::Null) => {
                fields.field("null");
            }
            Step::Node(Document::Bool(flag)) => {
                fields.field("boolean").field(flag.to_string());
            }
            Step::Node(Document::Number(Number::PosInt(number))) => {
                fields.field("positive-integer").field(number.to_string());
            }
            Step::Node(Document::Number(Number::NegInt(number))) => {
                fields.field("negative-integer").field(number.to_string());
            }
            Step::Node(Document::Number(Number::Float(number))) => {
                // The kind field is what keeps a float apart from an integer of the same
                // value: the text below is `f64`'s own rendering, deterministic for a
                // given value but no canonical encoding of one (every NaN renders alike).
                fields.field("float").field(number.to_string());
            }
            Step::Node(Document::String(text)) => {
                fields.field("string").field(text.as_str());
            }
            // A collection announces how many children follow, and its children are
            // walked in order — an array's own order, which belongs to the value.
            Step::Node(Document::Array(items)) => {
                fields.field("array").field(items.len().to_string());
                pending.extend(items.iter().rev().map(Step::Node));
            }
            // An object's members are walked in SORTED KEY ORDER, because the order
            // the wire carried them in, and the order the map iterates them in, are
            // no part of the value.
            Step::Node(Document::Object(members)) => {
                fields.field("object").field(members.len().to_string());
                let mut sorted: Vec<(&String, &Document)> = members.iter().collect();
                sorted.sort_unstable_by_key(|(key, _)| *key);
                for (key, member) in sorted.into_iter().rev() {
                    // Pushed last-first, so the key is popped before its value.
                    pending.push(Step::Node(member));
                    pending.push(Step::Key(key.as_str()));
                }
            }
        }
    }
}

/// The workflow configuration a request supplies, borrowed for digesting: the
/// members of the configuration a version freezes ([`WorkflowSnapshot`]) that a
/// request can express.
///
/// An operation whose request shape carries no member for part of the
/// configuration leaves that field `None`, so what is digested is always the
/// configuration the request itself supplies.
#[derive(Debug, Clone, Copy)]
pub struct WorkflowConfigurationView<'a> {
    pub role_arn: &'a str,
    pub description: Option<&'a str>,
    pub trigger_mode: Option<&'a str>,
    pub engine_version: Option<i32>,
    pub definition_s3_location: &'a DefinitionS3Location,
    pub code: Option<&'a Code>,
    pub encryption_configuration: Option<&'a EncryptionConfiguration>,
    pub logging_configuration: Option<&'a LoggingConfiguration>,
    pub network_configuration: Option<&'a NetworkConfiguration>,
}

impl WorkflowConfigurationView<'_> {
    /// Append this configuration to `fields` as ONE group, walked in the ONE
    /// order every workflow digest walks it in: the role, description, trigger
    /// mode and engine version, the definition object, the code union, the
    /// encryption configuration, the logging configuration and the network
    /// configuration's two id lists.
    ///
    /// Every member the request can omit, and every list, is a [`DigestFields`]
    /// group that announces its own extent, so a member's fields can never be
    /// read as another member's: two configurations that differ in any member,
    /// in which members they supply at all, or in the size or order of a list,
    /// never share a digest. This is the single derivation every operation that
    /// digests a workflow configuration drives, so a member added here reaches
    /// all of them.
    pub fn digest_group(&self, fields: &mut DigestFields) {
        let engine_version = self.engine_version.map(|version| version.to_string());
        fields.group(|configuration| {
            configuration
                .field(self.role_arn)
                .optional(self.description)
                .optional(self.trigger_mode)
                .optional(engine_version.as_deref())
                .group(|definition| {
                    definition
                        .field(self.definition_s3_location.bucket.as_str())
                        .field(self.definition_s3_location.object_key.as_str())
                        .optional(self.definition_s3_location.version_id.as_deref());
                })
                .group(|code| match self.code {
                    Some(Code::S3Location(location)) => {
                        code.field("S3Location")
                            .field(location.bucket.as_str())
                            .field(location.object_key.as_str())
                            .optional(location.version_id.as_deref());
                    }
                    None => {}
                })
                .group(|encryption| {
                    if let Some(supplied) = self.encryption_configuration {
                        encryption
                            .field(supplied.r#type.as_str())
                            .optional(supplied.kms_key_id.as_deref());
                    }
                })
                .group(|logging| {
                    if let Some(supplied) = self.logging_configuration {
                        logging.field(supplied.log_group_name.as_str());
                    }
                })
                .group(|network| {
                    if let Some(supplied) = self.network_configuration {
                        network
                            .optional_list(
                                supplied
                                    .security_group_ids
                                    .as_deref()
                                    .map(|ids| ids.iter().map(String::as_str)),
                            )
                            .optional_list(
                                supplied
                                    .subnet_ids
                                    .as_deref()
                                    .map(|ids| ids.iter().map(String::as_str)),
                            );
                    }
                });
        });
    }
}

/// The `ResourceType` an error document of this service reports for a workflow.
///
/// A type token names the resource CLASS by its AWS resource name — the capitalised
/// entity name, exactly as the recorded service answers it (fixture
/// `mwaa-serverless-resource-not-found-errors`: `UpdateWorkflow`,
/// `ListTagsForResource`, `TagResource` and `UntagResource` each report `Workflow`
/// for a workflow nothing holds) — and never a lowercased or hyphenated spelling of
/// it, so a client matching on the class sees the token AWS publishes. The model's own
/// name for the class is the same word: its resource shape is `WorkflowResource`,
/// identified by `WorkflowArn`
/// (`smithy_query(query="shape", shape="WorkflowResource")`).
pub const WORKFLOW_RESOURCE_TYPE: &str = "Workflow";

/// The `ResourceType` for one version of a workflow, the class named as the model
/// names it (`WorkflowVersion` / `WorkflowVersionSummary`,
/// `smithy_query(query="shape", shape="WorkflowVersion,WorkflowVersionSummary")`).
pub const WORKFLOW_VERSION_RESOURCE_TYPE: &str = "WorkflowVersion";

/// The `ResourceType` for one run of a workflow, the class named as the model names it
/// (`WorkflowRunSummary`, `smithy_query(query="shape", shape="WorkflowRunSummary")`).
pub const WORKFLOW_RUN_RESOURCE_TYPE: &str = "WorkflowRun";

/// The `ResourceType` for one task instance of a run, the class named as the model
/// names it (`TaskInstanceSummary`,
/// `smithy_query(query="shape", shape="TaskInstanceSummary")`).
pub const TASK_INSTANCE_RESOURCE_TYPE: &str = "TaskInstance";

/// The workflow's OWN identifier, taken out of a `WorkflowArn`: the resource-id
/// segment the modelled pattern places after `workflow/`
/// (`[a-zA-Z0-9][a-zA-Z0-9.\-_]{0,254}-[a-zA-Z0-9]{10}`,
/// `smithy_query(query="shape", shape="WorkflowArn")`).
///
/// A refusal reports the identifier OF the resource, not the reference a request
/// addressed it by, so an operation that takes a `WorkflowArn` reports this segment
/// rather than the whole ARN — a client comparing it against the id it tracks
/// matches, and one re-composing an ARN from it does not nest a second ARN inside.
/// A value carrying no `/` has no segment to take and is reported as it stands.
pub fn workflow_resource_id(workflow_arn: &str) -> &str {
    workflow_arn
        .rsplit_once('/')
        .map_or(workflow_arn, |(_, resource_id)| resource_id)
}

/// Mint a `WorkflowArn`: `arn:aws:airflow-serverless:<region>:<account>:workflow/<name>-<10 alnum>`,
/// the shape the modelled `WorkflowArn` pattern requires. The 10-character suffix
/// is the service-generated unique part the docs describe ("The service generates
/// a unique identifier that is appended").
pub fn mint_workflow_arn(region: &str, name: &str, nonce: &str) -> String {
    let suffix: String = digest(&["workflow-arn", name, nonce])
        .chars()
        .take(10)
        .collect();
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("workflow/{name}-{suffix}"))
}

/// Mint a `WorkflowVersion`: 32 lowercase hex characters, per the modelled
/// `^[a-z0-9]{32}$`. Derived from the workflow's identity and the configuration
/// the version freezes, so two different configurations never share a version id.
pub fn mint_workflow_version(workflow_arn: &str, fingerprint: &str, nonce: &str) -> String {
    digest(&["workflow-version", workflow_arn, fingerprint, nonce])
        .chars()
        .take(32)
        .collect()
}

/// Mint a `RevisionId` for a workflow configuration revision: a canonical
/// 36-character hyphenated UUID, the identifier form the recorded service answers this
/// member with — a shape class the 32-character lowercase-hex `WorkflowVersion`
/// ([`mint_workflow_version`]) deliberately does NOT share, so a client that tells a
/// revision from a version by its form can.
///
/// `RevisionId` is a bare `String` in the model with no length or pattern to satisfy
/// (`smithy_query(query="shape", shape="CreateWorkflowResponse")`), so the form comes
/// from the recorded answer rather than from a constraint.
///
/// Derived from the workflow's identity and the version it belongs to, so it changes
/// whenever the version does — which is what makes it usable "for optimistic concurrency
/// control in update operations" — and two reads of one version report the same revision.
/// The 16 bytes handed over are the leading bytes of that derivation; the textual form is
/// `service_common`'s, the tree's one rendering of a UUID, so the group boundaries and the
/// version and variant bits are not spelled out again here.
pub fn mint_revision_id(workflow_arn: &str, version: &str) -> String {
    ::service_common::uuid_from_random_bytes(digest_bytes(&[
        "workflow-revision",
        workflow_arn,
        version,
    ]))
}

/// The leading 16 bytes of the SHA-256 digest over `fields`, each field length-delimited
/// exactly as [`digest`] delimits them, so the two derivations agree on what a field is.
///
/// What it is for: an identifier whose FORM is a UUID is rendered from bytes rather than
/// from hex text ([`mint_revision_id`]).
fn digest_bytes(fields: &[&str]) -> [u8; 16] {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update(field.len().to_le_bytes());
        hasher.update(field.as_bytes());
    }
    let digested = hasher.finalize();
    let mut bytes = [0u8; 16];
    // `copy_from_slice` over two spans of the same length: SHA-256 is 32 bytes, so the
    // leading 16 are always there.
    bytes.copy_from_slice(&digested[..16]);
    bytes
}

/// Mint a `RunId` matching the modelled `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`.
pub fn mint_run_id(workflow_arn: &str, nonce: &str) -> String {
    let suffix: String = digest(&["workflow-run", workflow_arn, nonce])
        .chars()
        .take(16)
        .collect();
    format!("run-{suffix}")
}

/// Mint the `TaskInstanceId` of the instance ONE run records for one attempt at one
/// declared task: the run the instance belongs to, the task it executes and the attempt it
/// is, composed into one identifier the modelled `IdString`
/// (`^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`, 1..255) carries.
///
/// An instance identifier is the service's own, not the definition's: `TaskInstanceId` is
/// "The unique identifier of this task instance" while the task the instance executes is
/// reported separately as `TaskId`, "The unique identifier of the task definition within
/// the workflow" (`smithy_query(query="shape",
/// shape="TaskInstanceSummary,GetTaskInstanceResponse")`). So the identifier is minted for
/// the INSTANCE — one per (run, task, attempt) — and two runs of one definition never
/// report the same instance identifier, which is what makes the identifier a per-run handle
/// a client can key a store by.
///
/// What an instance IS is exactly that triple: the execution of one declared task, inside
/// one run, on one attempt. The identifier names it by naming all three, which is the form
/// the recorded service answers this member with, and is also why the modelled `IdString`
/// admits the `.` that separates them. Three values already inside that member's character
/// class compose into one that is inside it too, so only the LENGTH of the composition can
/// take it outside what the member reports — a definition may declare a task identifier as
/// long as the member itself — and the composition is held to the modelled length
/// ([`id_within_modelled_length`]).
///
/// Composed rather than drawn at random, so recomputing it for the same instance always
/// lands on the same value: a run materialises its execution whenever a reader advances the
/// clock ([`WorkflowRun::settle`]), and an identifier that changed between two of those
/// passes would record one instance twice.
pub fn mint_task_instance_id(run_id: &str, task_id: &str, attempt: i32) -> String {
    id_within_modelled_length(format!("{run_id}.{task_id}.{attempt}"))
}

/// The most characters a modelled `IdString` member carries: `@length(min: 1, max: 255)`
/// (`smithy_query(query="shape", shape="IdString")`), the shape `TaskInstanceId`, `TaskId`,
/// `RunId` and `LogStream` all target.
pub const MAX_ID_STRING_CHARS: usize = 255;

/// How many characters of digest a composed identifier carries once it had to be cut
/// ([`id_within_modelled_length`]).
const ID_DIGEST_CHARS: usize = 16;

/// `name`, held inside what a modelled `IdString` member can carry: the name as it stands
/// when it already fits, and otherwise its leading characters with a digest of the WHOLE
/// name appended, cut on a character boundary.
///
/// What it is for: this service names a thing a run produced by naming the things it
/// belongs to — the instance of a task in a run ([`mint_task_instance_id`]), the stream
/// that instance's attempt wrote to ([`run_logs::task_log_stream`]) — and part of such a
/// composition is caller-supplied definition content that may be as long as the member
/// reporting the whole. A name past the modelled length is therefore CUT rather than
/// reported unreportably, and the digest is what keeps a cut name naming ONE thing: two
/// compositions agreeing on their leading characters still differ in it.
pub fn id_within_modelled_length(name: String) -> String {
    if name.chars().count() <= MAX_ID_STRING_CHARS {
        return name;
    }
    let digested: String = digest(&["composed-identifier", &name])
        .chars()
        .take(ID_DIGEST_CHARS)
        .collect();
    let head: String = name
        .chars()
        .take(MAX_ID_STRING_CHARS - ID_DIGEST_CHARS - 1)
        .collect();
    format!("{head}.{digested}")
}

/// The marker every pagination cursor this service issues carries, so a token it
/// did not mint (or one minted for a different list) is refused rather than
/// silently restarting the walk somewhere.
const PAGE_TOKEN_PREFIX: &str = "mwaaserverless.page.v1:";

/// Encode a pagination cursor so a client cannot read or construct one by hand:
/// the kind of list plus the key of the last item on the page, hex-encoded.
pub fn encode_page_token(kind: &str, last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{kind}\u{1f}{last_key}"))
}

/// Decode a pagination cursor [`encode_page_token`] issued for `kind`. `None`
/// when the token was not issued by this service for this list, which every
/// caller refuses as a `ValidationException`.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let decoded = String::from_utf8(bytes).ok()?;
    let rest = decoded.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let (token_kind, key) = rest.split_once('\u{1f}')?;
    (token_kind == kind).then(|| key.to_string())
}

/// The smallest page size the modelled `MaxResults` range allows. Every paginated
/// request of this service declares that member `@range(min: 1, max: 100)` with
/// `@default: 20` (`smithy_query(query="shape", shape="ListWorkflowsRequest,
/// ListWorkflowRunsRequest,ListWorkflowVersionsRequest,ListTaskInstancesRequest")`).
pub const MIN_PAGE_SIZE: usize = 1;

/// The largest page size that same range allows.
pub const MAX_PAGE_SIZE: usize = 100;

/// The page size one walk of a paginated listing serves, from the `MaxResults` the
/// request carries — the single derivation every list handler of this service
/// reads its page size through, beside the cursor helpers they already share.
///
/// `MaxResults` carries `@default: 20` and `@range(1..100)`, both applied before a
/// handler runs, so the value is already a usable page size; it is held to those
/// published bounds anyway, because a page size of zero would report "there is
/// nothing here" for a parent that holds items — a wrong answer dressed as success
/// — and a walk must always make progress.
pub fn page_size(max_results: i32) -> usize {
    usize::try_from(max_results)
        .unwrap_or(MIN_PAGE_SIZE)
        .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE)
}

/// What separates a listing cursor key's POSITION from the SCOPE of the walk it
/// was issued for. Neither a zero-padded decimal position, a `WorkflowArn`
/// (`^arn:…:workflow/[a-zA-Z0-9.\-_]+$`) nor a region segment
/// (`[a-z]{2}-[a-z]+-[0-9]{1}`) can contain it, so the split is unambiguous.
const LISTING_KEY_SCOPE_SEPARATOR: char = '@';

/// The cursor key for a position in ONE listing walk: the position of the last
/// item on the page, and the SCOPE that walk is over — the parent workflow for a
/// child listing, the endpoint's region for a listing partitioned by region.
///
/// A walk covers exactly one scope, so the scope belongs in the key: a cursor
/// presented on a walk over ANOTHER scope is then recognisable as one not issued
/// for that walk, instead of being applied as a position in a sequence it says
/// nothing about.
pub fn format_scoped_listing_key(scope: &str, position: &str) -> String {
    format!("{position}{LISTING_KEY_SCOPE_SEPARATOR}{scope}")
}

/// The POSITION a cursor of `kind` carries, for a walk over `scope`.
///
/// `None` when the token is not one this service issued, when it was issued for
/// another of this service's lists, when its key is malformed, or when it was
/// issued on another scope's walk — each of which a listing refuses with
/// the same `ValidationException` ([`invalid_page_token`]), because none of them
/// is a position in the walk being asked for.
pub fn parse_scoped_page_token(kind: &str, token: &str, scope: &str) -> Option<String> {
    let key = decode_page_token(kind, token)?;
    let (position, scope_of_token) = key.split_once(LISTING_KEY_SCOPE_SEPARATOR)?;
    (scope_of_token == scope).then(|| position.to_string())
}

/// The cursor key for a position in the listing of the workflows ONE regional
/// endpoint holds: the `WorkflowArn` of the last workflow on the page, scoped to
/// the region whose partition that walk is over.
///
/// The ARN rather than an index, so a workflow removed between pages leaves the
/// cursor resolving to the same place in what remains; the region beside it
/// because the workflow listing is partitioned by region ([`request_region`]),
/// so a position taken from another endpoint's walk is not a position in this one.
pub fn format_workflow_listing_key(region: &str, last_workflow_arn: &str) -> String {
    format_scoped_listing_key(region, last_workflow_arn)
}

/// The ARN a cursor of `kind` resumes `region`'s workflow listing after, as
/// [`format_workflow_listing_key`] wrote it.
///
/// `None` when the token is not one this service issued for this list, when its
/// key is malformed, or when it was issued at ANOTHER regional endpoint — each of
/// which the listing refuses with [`invalid_page_token`], because none of them is
/// a position in the walk being asked for.
pub fn parse_workflow_listing_page_token(kind: &str, token: &str, region: &str) -> Option<String> {
    parse_scoped_page_token(kind, token, region).filter(|position| !position.is_empty())
}

/// What separates the workflow from the version filter in the scope a run listing's
/// cursor is bound to. A `WorkflowArn` carries exactly one `/`, before a resource id
/// of `[a-zA-Z0-9][a-zA-Z0-9.\-_]{0,254}-[a-zA-Z0-9]{10}` (`smithy_query(query="shape",
/// shape="WorkflowArn")`), so this infix cannot be part of the workflow it follows;
/// it spells the selector the way the operation's own URI spells a version.
const VERSION_FILTER_SCOPE_INFIX: &str = "/versions/";

/// The scope suffix for the walk over EVERY version's runs — the one a request that
/// names no version filter asks for.
const UNFILTERED_VERSION_SCOPE: &str = "-";

/// What a scope narrowed to ONE version writes before the version itself. A version
/// selector is `^.+$`, i.e. any non-empty text, so the filtered and unfiltered forms
/// are told apart by this leading marker rather than by the version's own characters:
/// no filtered scope can read as the unfiltered one, whatever version is named.
const NAMED_VERSION_SCOPE_MARKER: char = '=';

/// The walk a run-listing cursor belongs to: the workflow whose runs it pages, AND the
/// version filter that selects WHICH of that workflow's runs the walk reports.
///
/// A run listing's set is narrowed by a request member as well as by its parent — "If
/// not specified, runs for all versions are returned"
/// (`smithy_query(query="operation", operation="ListWorkflowRuns")`,
/// `WorkflowVersion`) — so that filter is part of the scope: a cursor minted under one
/// filter is then recognisable as not a position in the walk a request under another
/// filter asks for, instead of being applied as a position in a sequence it says
/// nothing about and silently dropping the runs that sort before it.
fn run_listing_scope(workflow_arn: &str, workflow_version: Option<&str>) -> String {
    let filter = match workflow_version {
        None => UNFILTERED_VERSION_SCOPE.to_string(),
        Some(version) => format!("{NAMED_VERSION_SCOPE_MARKER}{version}"),
    };
    format!("{workflow_arn}{VERSION_FILTER_SCOPE_INFIX}{filter}")
}

/// The cursor key for a position in ONE workflow's run listing under ONE version
/// filter: the two components of a [`RunListingKey`], zero-padded so the key a cursor
/// carries is fixed-width and orders the same way the tuple does, scoped to the walk
/// it was issued for ([`run_listing_scope`]).
pub fn format_run_listing_key(
    workflow_arn: &str,
    workflow_version: Option<&str>,
    key: RunListingKey,
) -> String {
    let (nanos, sequence) = key;
    format_scoped_listing_key(
        &run_listing_scope(workflow_arn, workflow_version),
        &format!("{nanos:039}.{sequence:020}"),
    )
}

/// The POSITION a cursor of `kind` carries for the walk over ONE workflow's runs under
/// ONE version filter, as [`format_run_listing_key`] wrote it.
///
/// `None` when the token is not one this service issued, when it was issued for
/// another of its lists, when it was issued on another workflow's runs or under
/// another version filter, or when its key is malformed — each of which the listing
/// refuses with [`invalid_page_token`], because none of them is a position in the walk
/// being asked for.
pub fn parse_run_listing_page_token(
    kind: &str,
    token: &str,
    workflow_arn: &str,
    workflow_version: Option<&str>,
) -> Option<RunListingKey> {
    let position =
        parse_scoped_page_token(kind, token, &run_listing_scope(workflow_arn, workflow_version))?;
    parse_run_listing_key(&position)
}

/// Read back the position part of a key [`format_run_listing_key`] wrote, as
/// [`parse_scoped_page_token`] hands it over. `None` for anything else, which a
/// listing refuses as a `ValidationException` just like a token it did not issue.
fn parse_run_listing_key(position: &str) -> Option<RunListingKey> {
    let (nanos, sequence) = position.split_once('.')?;
    Some((nanos.parse().ok()?, sequence.parse().ok()?))
}

/// The cursor key for a position in ONE workflow's version listing: the
/// [`WorkflowVersionRecord::sequence`] of the last version on the page,
/// zero-padded so the key is fixed-width, scoped to the workflow the walk is over.
///
/// The POSITION rather than the version's identifier, so a cursor still resolves
/// to the same place once the version it was issued at has been deleted.
pub fn format_version_listing_key(workflow_arn: &str, sequence: u64) -> String {
    format_scoped_listing_key(workflow_arn, &format!("{sequence:020}"))
}

/// Read back the position part of a key [`format_version_listing_key`] wrote, as
/// [`parse_scoped_page_token`] hands it over. `None` for anything else, which a
/// listing refuses as a `ValidationException` just like a token it did not issue.
///
/// A version listing's position is a single number, so this is only the pair of
/// [`format_version_listing_key`] — named so the version listing reads its cursor
/// through the same two steps the run listing does, not because the position has a
/// format of its own.
pub fn parse_version_listing_key(position: &str) -> Option<u64> {
    position.parse().ok()
}

/// What separates the workflow from the run in the scope a task-instance
/// listing's cursor is bound to. A `WorkflowArn` already contains `/`, but the
/// scope is only ever compared whole, so any separator a `RunId`
/// (`^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`) cannot contain keeps two runs' scopes
/// distinct; this one spells the scope the way the operation's own URI does
/// (`/workflows/{WorkflowArn}/runs/{RunId}/tasks`).
const RUN_SCOPE_INFIX: &str = "/runs/";

/// The walk a task-instance cursor belongs to: the RUN whose task instances it
/// pages, inside the workflow that holds that run.
///
/// A task-instance listing pages the instances of exactly one run, so the run —
/// not just its workflow — is what a cursor is a position in: a cursor minted
/// while walking ANOTHER run's instances is then recognisable as one not issued
/// for this walk.
fn task_instance_listing_scope(workflow_arn: &str, run_id: &str) -> String {
    format!("{workflow_arn}{RUN_SCOPE_INFIX}{run_id}")
}

/// The cursor key for a position in ONE run's task-instance listing: the
/// identifier of the last instance on the page, scoped to the run the walk is
/// over.
///
/// A run holds its instances keyed by `TaskInstanceId`
/// ([`WorkflowRun::task_instances`]) and a listing walks them in that key's
/// order, so the identifier IS the position: resuming after it reports exactly
/// the instances that order places later, and an instance removed between pages
/// leaves the cursor resolving to the same place in what remains.
pub fn format_task_instance_listing_key(
    workflow_arn: &str,
    run_id: &str,
    last_task_instance_id: &str,
) -> String {
    format_scoped_listing_key(
        &task_instance_listing_scope(workflow_arn, run_id),
        last_task_instance_id,
    )
}

/// The POSITION a cursor of `kind` carries for the walk over ONE run's task
/// instances, as [`format_task_instance_listing_key`] wrote it.
///
/// `None` when the token is not one this service issued, when it was issued for
/// another of its lists, when it was issued on another run's instances, or when
/// it carries no position at all — each of which the listing refuses with
/// [`invalid_page_token`], because none of them is a position in the walk being
/// asked for.
pub fn parse_task_instance_page_token(
    kind: &str,
    token: &str,
    workflow_arn: &str,
    run_id: &str,
) -> Option<String> {
    parse_scoped_page_token(
        kind,
        token,
        &task_instance_listing_scope(workflow_arn, run_id),
    )
    .filter(|position| !position.is_empty())
}

/// The HTTP status this service answers a resource-lookup miss with.
///
/// The model gives `ResourceNotFoundException` `@httpError 404`
/// (`smithy_query shape=ResourceNotFoundException`), but the recorded service answers
/// a lookup for a workflow/version/run it does not hold with **HTTP 400** carrying
/// that same shape (fixture `mwaa-serverless-resource-not-found-errors`: GetWorkflow,
/// DeleteWorkflow, StartWorkflowRun and GetWorkflowRun each answer
/// `400 ResourceNotFoundException`). The recording is what a client sees, so the
/// shape keeps its modelled identity and only the status is overridden, through
/// [`crate::scaffolded::Scaffolded::ModeledWithStatus`].
pub const LOOKUP_MISS_STATUS: u16 = 400;

/// The entity an operation could not resolve.
///
/// The refusal names the entity THAT OPERATION had to resolve to do its work, which
/// is why `StartWorkflowRun` reports the missing workflow VERSION and `GetWorkflowRun`
/// the missing RUN for the very same absent workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedEntity {
    /// The workflow a `WorkflowArn` names.
    Workflow,
    /// The workflow's latest version, selected when a request names no
    /// `WorkflowVersion`.
    LatestWorkflowVersion,
    /// The version a `WorkflowVersion` names.
    NamedWorkflowVersion,
    /// The run a `RunId` names.
    WorkflowRun,
    /// The task instance a `TaskInstanceId` names, inside the run that holds it.
    TaskInstance,
}

impl UnresolvedEntity {
    /// The version entity a request selects: the one it named, or the latest.
    pub fn version_for(requested: Option<&str>) -> Self {
        if requested.is_some() {
            Self::NamedWorkflowVersion
        } else {
            Self::LatestWorkflowVersion
        }
    }

    /// What the refusal says. Entity-level and free of request values, so a client
    /// can tell "workflow absent" from "no version to run" from "run absent".
    pub fn message(self) -> &'static str {
        match self {
            Self::Workflow => "Resource not found",
            Self::LatestWorkflowVersion => "Unable to find latest version",
            Self::NamedWorkflowVersion => "Workflow version not found",
            Self::WorkflowRun => "Workflow run not found",
            // Not in any recording of this service (no fixture reads a task
            // instance); entity-level and free of request values, like the others,
            // so a client can tell which entity failed to resolve.
            Self::TaskInstance => "Task instance not found",
        }
    }

    /// The `ResourceType` the refusal reports for this entity: the AWS resource name
    /// of its class ([`WORKFLOW_RESOURCE_TYPE`] and the tokens beside it).
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Workflow => WORKFLOW_RESOURCE_TYPE,
            Self::LatestWorkflowVersion | Self::NamedWorkflowVersion => {
                WORKFLOW_VERSION_RESOURCE_TYPE
            }
            Self::WorkflowRun => WORKFLOW_RUN_RESOURCE_TYPE,
            Self::TaskInstance => TASK_INSTANCE_RESOURCE_TYPE,
        }
    }
}

/// The modelled payload for a reference that resolves to nothing: the entity-level
/// message, plus the `ResourceId`/`ResourceType` the shape requires.
///
/// Every handler in this service funnels its lookup misses through here (wrapped by
/// [`lookup_miss`]), so one status and one vocabulary cover them all.
///
/// `resource_id` is the IDENTIFIER of the entity that did not resolve — the workflow's
/// id segment ([`workflow_resource_id`]), a version id, a run id, a task-instance id —
/// so what the refusal reports is the resource, not the reference a request addressed
/// it by. The tag operations, which address an untyped taggable resource and are handed
/// nothing but its ARN, report that ARN.
pub fn unresolved(entity: UnresolvedEntity, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: entity.message().to_string(),
        resource_id: resource_id.to_string(),
        resource_type: entity.resource_type().to_string(),
    }
}

/// Serve a lookup miss the way the recorded service does: the modelled
/// `ResourceNotFoundException` the handler built, with [`LOOKUP_MISS_STATUS`].
pub fn lookup_miss<E>(error: E) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::modeled_with_status(error, LOOKUP_MISS_STATUS)
}

/// A `SystemTime` as the modelled `TimestampValue`.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

// ===========================================================================
// Request validation: the modelled `ValidationException` for a bad member value
// ===========================================================================
//
// Every operation of this service models `ValidationException` (HTTP 400) with
// `Message` and `Reason` @required and an optional `FieldList`
// (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionField")`).
// A member value that fails a constraint the model declares for it is therefore
// answered with that shape — a top-level `Message`, `Reason` =
// `fieldValidationFailed`, and one `FieldList` entry per offending member — and
// NOT with the framework's generic refusal envelope, which carries none of them.
//
// NOTHING about constraint checking is re-implemented here. The constraints a
// member's `skip_validation` strips off its generated member schema are
// RE-DECLARED below in the framework's own representation — a
// [`TraitMap`](::aws_smithy_schema::TraitMap) of
// [`LengthTrait`]/[`PatternTrait`]/[`EnumTrait`]/[`ElementConstraintsTrait`], the
// exact form `vendor/smithy-macros` would have attached — hung on a member schema
// of the same name, and the check itself is
// [`::aws_smithy_schema_server::constraints::validate`], the SAME walk the request
// boundary runs. So the clause, the elision of a long value, the violation cap and
// the lowerCamel path are all the framework's, and a request refused here reads
// identically to one refused at the boundary.
//
// An operation declares `skip_validation = [...]` on its input struct in
// `src/ops/types/` and hands the matching member schemas from this module to
// [`validate_members`]; its violations become the modelled error through
// [`constraint_validation_exception`].
//
// The ARN-typed members are the one class whose REPORTING is this service's own
// rather than the framework's: a request's ARNs are parsed by the service's own
// ARN validators, which state their own field name and message. The acceptance
// test is still the modelled `@pattern`, run by the same walk; only the refusal
// is worded by this service. See [`arn_validation_exception`].
//
// ONE member is deliberately NOT covered: the paginated operations' `MaxResults`
// and its `@range(1..100)`. The recorded AWS answer for an out-of-range page size
// is the request-validation layer's own envelope — a 400 whose body carries a
// lowercase `message` and neither `Reason` nor `FieldList` — so that bound is
// left where the framework boundary already refuses it, exactly as recorded.

/// The `ValidationExceptionReason` a bad member value carries.
///
/// `smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `unknownOperation | cannotParse | fieldValidationFailed | other`; a value
/// that failed the constraint declared for its member is the third.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The `ValidationExceptionReason` a request carries when every member value it
/// holds is within its declared constraint and the request is still refused
/// because it is "inconsistent with … service requirements"
/// (`smithy_query(query="shape", shape="ValidationException")`).
///
/// `smithy_query(query="shape", shape="ValidationExceptionReason")` lists
/// `unknownOperation | cannotParse | fieldValidationFailed | other`; a refusal
/// that no member of the request is at fault for is the fourth.
pub const REASON_OTHER: &str = "other";

/// This service's Smithy namespace, for the shadow schemas below.
const NAMESPACE: &str = "com.amazonaws.mwaaserverless";

/// The workflow-ARN `@pattern`, shared by `WorkflowArn` and
/// `TaggableResourceArn` (`smithy_query(query="shape", shape="WorkflowArn")`).
const WORKFLOW_ARN_PATTERN: &str = r"^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:airflow-serverless:([a-z]{2}-[a-z]+-[0-9]{1}):([0-9]{12}):workflow/([a-zA-Z0-9][a-zA-Z0-9\.\-_]{0,254}-[a-zA-Z0-9]{10})$";

/// The identifier `@pattern`, shared by `NameString` and `IdString`.
const IDENTIFIER_PATTERN: &str = r"^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$";

/// `WorkflowArn`: `@length(1..2048)` plus the workflow-ARN `@pattern`
/// (`smithy_query(query="shape", shape="WorkflowArn")`).
static WORKFLOW_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(WORKFLOW_ARN_PATTERN)));
    traits
});

/// `TaggableResourceArn`: `@length(1..1011)` plus the workflow-ARN `@pattern`.
static TAGGABLE_RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1011))));
    traits.insert(Box::new(PatternTrait::new(WORKFLOW_ARN_PATTERN)));
    traits
});

/// `RoleARN`: `@length(1..2048)` plus the IAM role-ARN `@pattern`.
static ROLE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(
        r"^arn:aws(?:-(?:cn|us-gov|iso|iso-b|iso-e|iso-f))?:iam::[0-9]{12}:role(/[a-zA-Z0-9+=,.@_\-]{1,512})*?/[a-zA-Z0-9+=,.@_\-]{1,64}$",
    )));
    traits
});

/// `NameString`: `@length(1..255)` plus the identifier `@pattern`.
static NAME_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(IDENTIFIER_PATTERN)));
    traits
});

/// `IdString`: `@length(1..255)` plus the identifier `@pattern`.
static ID_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(IDENTIFIER_PATTERN)));
    traits
});

/// `IdempotencyTokenString`: `@length(1..128)` plus printable-ASCII `@pattern`.
static IDEMPOTENCY_TOKEN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(r"^[\x21-\x7E]+$")));
    traits
});

/// `DescriptionString`: `@length(1..1024)` plus `@pattern(^.+$)`.
static DESCRIPTION_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1024))));
    traits.insert(Box::new(PatternTrait::new(r"^.+$")));
    traits
});

/// `GenericString`: `@length(1..255)` plus `@pattern(^.*$)`.
static GENERIC_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});

/// `VersionId`: `@length(1..2048)` plus `@pattern(^.+$)`.
static VERSION_ID_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(2048))));
    traits.insert(Box::new(PatternTrait::new(r"^.+$")));
    traits
});

/// `WorkflowVersion`: `@length(32..32)` plus `@pattern(^[a-z0-9]{32}$)`.
static WORKFLOW_VERSION_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(32), Some(32))));
    traits.insert(Box::new(PatternTrait::new(r"^[a-z0-9]{32}$")));
    traits
});

/// `TagKey`: `@length(1..128)` plus `@pattern(^.*$)`.
static TAG_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});

/// `TagValue`: `@length(0..256)` plus `@pattern(^.*$)`.
static TAG_VALUE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(0), Some(256))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});

/// `SecurityGroupString`: `@length(1..255)` plus `@pattern(^sg-[a-z0-9]*$)`.
static SECURITY_GROUP_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(r"^sg-[a-z0-9]*$")));
    traits
});

/// `SubnetString`: `@length(1..255)` plus `@pattern(^subnet-[a-z0-9]*$)`.
static SUBNET_STRING_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(255))));
    traits.insert(Box::new(PatternTrait::new(r"^subnet-[a-z0-9]*$")));
    traits
});

/// `S3Location$Bucket`: `@length(3..63)`, no pattern
/// (`smithy_query(query="shape", shape="S3Location")`).
static S3_BUCKET_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(3), Some(63))));
    traits
});

/// `S3Location$ObjectKey`: `@length(1..1024)`, no pattern.
static S3_OBJECT_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1024))));
    traits
});

/// `S3Location$VersionId`: `@length(1..1024)`, no pattern.
static S3_VERSION_ID_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(1024))));
    traits
});

/// `EncryptionConfiguration$Type`: the `EncryptionType` value set.
static ENCRYPTION_TYPE_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(EnumTrait::new(ENCRYPTION_TYPE_VALUES)));
    traits
});

/// `Tags`: `@length(0..50)` ENTRIES, with `TagValue`'s own constraints carried
/// for each value exactly as codegen carries them
/// (`smithy_query(query="shape", shape="Tags")`).
///
/// The walk does not run a constraint on a map KEY: a key is written against the
/// map member's own schema, whose trait map describes the MAP (its entry count)
/// and its VALUES. The key shape's constraints are therefore carried separately,
/// by [`M_TAGS_KEY`], and handed to the same walk beside the map itself by
/// [`constrained_tags`].
static TAGS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(0), Some(50))));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TAG_VALUE_TRAITS)));
    traits
});

/// `TagKeys`: `@length(1..50)` ENTRIES, with `TagKey`'s own constraints carried
/// for each element (`smithy_query(query="shape", shape="TagKeys")`).
static TAG_KEYS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(50))));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TAG_KEY_TRAITS)));
    traits
});

/// `SecurityGroupIds`: no entry-count bound; each element carries
/// `SecurityGroupString`'s constraints.
static SECURITY_GROUP_IDS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(ElementConstraintsTrait::new(
        &SECURITY_GROUP_STRING_TRAITS,
    )));
    traits
});

/// `SubnetIds`: no entry-count bound; each element carries `SubnetString`'s
/// constraints.
static SUBNET_IDS_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(ElementConstraintsTrait::new(&SUBNET_STRING_TRAITS)));
    traits
});

// --- The member schemas the walk reads ------------------------------------
//
// One per (member name, target shape) pair this service hands to the handler,
// shared across every operation that declares that member: the walk reads only
// a member schema's NAME (for the violation path) and its TRAIT MAP, so one
// declaration serves every operation whose model gives that member the same
// target shape. `WorkflowVersion` appears twice because two operations give it
// two different target shapes.

/// `WorkflowArn`, as `CreateWorkflow`'s sibling operations declare it.
pub static M_WORKFLOW_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$WorkflowArn",
        NAMESPACE,
        "WorkflowArn",
    ),
    ShapeType::String,
    "WorkflowArn",
    0,
)
.with_traits(&WORKFLOW_ARN_TRAITS);

/// `ResourceArn`, which targets `TaggableResourceArn`.
pub static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$ResourceArn",
        NAMESPACE,
        "ResourceArn",
    ),
    ShapeType::String,
    "ResourceArn",
    1,
)
.with_traits(&TAGGABLE_RESOURCE_ARN_TRAITS);

/// `RoleArn`, which targets `RoleARN`.
pub static M_ROLE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$RoleArn",
        NAMESPACE,
        "RoleArn",
    ),
    ShapeType::String,
    "RoleArn",
    2,
)
.with_traits(&ROLE_ARN_TRAITS);

/// `Name`, which targets `NameString`.
pub static M_NAME: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Name",
        NAMESPACE,
        "Name",
    ),
    ShapeType::String,
    "Name",
    3,
)
.with_traits(&NAME_STRING_TRAITS);

/// `ClientToken`, which targets `IdempotencyTokenString`.
pub static M_CLIENT_TOKEN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$ClientToken",
        NAMESPACE,
        "ClientToken",
    ),
    ShapeType::String,
    "ClientToken",
    4,
)
.with_traits(&IDEMPOTENCY_TOKEN_TRAITS);

/// `Description`, which targets `DescriptionString`.
pub static M_DESCRIPTION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Description",
        NAMESPACE,
        "Description",
    ),
    ShapeType::String,
    "Description",
    5,
)
.with_traits(&DESCRIPTION_STRING_TRAITS);

/// `TriggerMode`, which targets `GenericString`.
pub static M_TRIGGER_MODE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TriggerMode",
        NAMESPACE,
        "TriggerMode",
    ),
    ShapeType::String,
    "TriggerMode",
    6,
)
.with_traits(&GENERIC_STRING_TRAITS);

/// `RunId`, which targets `IdString`.
pub static M_RUN_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$RunId",
        NAMESPACE,
        "RunId",
    ),
    ShapeType::String,
    "RunId",
    7,
)
.with_traits(&ID_STRING_TRAITS);

/// `TaskInstanceId`, which targets `IdString`.
pub static M_TASK_INSTANCE_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TaskInstanceId",
        NAMESPACE,
        "TaskInstanceId",
    ),
    ShapeType::String,
    "TaskInstanceId",
    8,
)
.with_traits(&ID_STRING_TRAITS);

/// `OperatorName`, which targets `GenericString`.
///
/// Unlike its siblings this member is not one a REQUEST carries: it is reported
/// out of the caller's workflow definition by `GetTaskInstance` and
/// `ListTaskInstances`, and the walk over this schema is what decides whether a
/// declared value is one that member can carry (see
/// [`workflow_execution`]).
pub static M_OPERATOR_NAME: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$OperatorName",
        NAMESPACE,
        "OperatorName",
    ),
    ShapeType::String,
    "OperatorName",
    17,
)
.with_traits(&GENERIC_STRING_TRAITS);

/// `WorkflowVersion` where the operation targets `WorkflowVersion` — a version
/// IDENTIFIER, exactly 32 characters (`GetWorkflow`, `DeleteWorkflow`).
pub static M_WORKFLOW_VERSION_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$WorkflowVersionId",
        NAMESPACE,
        "WorkflowVersion",
    ),
    ShapeType::String,
    "WorkflowVersion",
    9,
)
.with_traits(&WORKFLOW_VERSION_TRAITS);

/// `WorkflowVersion` where the operation targets `VersionId` — a version
/// SELECTOR, 1..2048 characters (`ListWorkflowRuns`, `StartWorkflowRun`).
pub static M_WORKFLOW_VERSION_SELECTOR: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$WorkflowVersionSelector",
        NAMESPACE,
        "WorkflowVersion",
    ),
    ShapeType::String,
    "WorkflowVersion",
    10,
)
.with_traits(&VERSION_ID_TRAITS);

/// `Tags`, the `Tags` map.
pub static M_TAGS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Tags",
        NAMESPACE,
        "Tags",
    ),
    ShapeType::Map,
    "Tags",
    11,
)
.with_traits(&TAGS_TRAITS);

/// The KEY position of a `Tags` map, which targets `TagKey` exactly as
/// `TagKeys`' element does (`smithy_query(query="shape", shape="Tags")`).
///
/// Named for the member that CARRIES the map, because that is the member a
/// request supplied the key in and the one a refusal must name; a key outside
/// `TagKey` is reported at that member's own path, the position the map's entry
/// count is reported at too.
///
/// Declared beside the map it belongs to, with the next free index of this
/// shadow family (the root struct's member list is empty, so the index only has
/// to be unique).
pub static M_TAGS_KEY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TagsKey",
        NAMESPACE,
        "Tags",
    ),
    ShapeType::String,
    "Tags",
    16,
)
.with_traits(&TAG_KEY_TRAITS);

/// `TagKeys`, the `TagKeys` list.
pub static M_TAG_KEYS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$TagKeys",
        NAMESPACE,
        "TagKeys",
    ),
    ShapeType::List,
    "TagKeys",
    12,
)
.with_traits(&TAG_KEYS_TRAITS);

/// `Code`, the union whose `S3Location` members carry the constraints.
pub static M_CODE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$Code",
        NAMESPACE,
        "Code",
    ),
    ShapeType::Union,
    "Code",
    13,
);

/// `EncryptionConfiguration`, whose `Type` carries the value set.
pub static M_ENCRYPTION_CONFIGURATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$EncryptionConfiguration",
        NAMESPACE,
        "EncryptionConfiguration",
    ),
    ShapeType::Structure,
    "EncryptionConfiguration",
    14,
);

/// `NetworkConfiguration`, whose two id lists carry their element constraints.
pub static M_NETWORK_CONFIGURATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput$NetworkConfiguration",
        NAMESPACE,
        "NetworkConfiguration",
    ),
    ShapeType::Structure,
    "NetworkConfiguration",
    15,
);

/// `Code$S3Location`.
pub static M_S3_LOCATION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#Code$S3Location",
        NAMESPACE,
        "S3Location",
    ),
    ShapeType::Structure,
    "S3Location",
    0,
);

/// `S3Location$Bucket`.
pub static M_BUCKET: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#S3Location$Bucket",
        NAMESPACE,
        "Bucket",
    ),
    ShapeType::String,
    "Bucket",
    0,
)
.with_traits(&S3_BUCKET_TRAITS);

/// `S3Location$ObjectKey`.
pub static M_OBJECT_KEY: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#S3Location$ObjectKey",
        NAMESPACE,
        "ObjectKey",
    ),
    ShapeType::String,
    "ObjectKey",
    1,
)
.with_traits(&S3_OBJECT_KEY_TRAITS);

/// `S3Location$VersionId`.
pub static M_S3_VERSION_ID: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#S3Location$VersionId",
        NAMESPACE,
        "VersionId",
    ),
    ShapeType::String,
    "VersionId",
    2,
)
.with_traits(&S3_VERSION_ID_TRAITS);

/// `EncryptionConfiguration$Type`.
pub static M_ENCRYPTION_TYPE: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#EncryptionConfiguration$Type",
        NAMESPACE,
        "Type",
    ),
    ShapeType::String,
    "Type",
    0,
)
.with_traits(&ENCRYPTION_TYPE_TRAITS);

/// `NetworkConfiguration$SecurityGroupIds`.
pub static M_SECURITY_GROUP_IDS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#NetworkConfiguration$SecurityGroupIds",
        NAMESPACE,
        "SecurityGroupIds",
    ),
    ShapeType::List,
    "SecurityGroupIds",
    0,
)
.with_traits(&SECURITY_GROUP_IDS_TRAITS);

/// `NetworkConfiguration$SubnetIds`.
pub static M_SUBNET_IDS: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#NetworkConfiguration$SubnetIds",
        NAMESPACE,
        "SubnetIds",
    ),
    ShapeType::List,
    "SubnetIds",
    1,
)
.with_traits(&SUBNET_IDS_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is
/// empty on purpose: the walk visits what [`ConstrainedMembers`] WRITES, which
/// is whichever members the calling operation handed over, and a root struct
/// schema carries no member name so it contributes no path segment.
static CONSTRAINED_INPUT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.mwaaserverless#ConstrainedInput",
        NAMESPACE,
        "ConstrainedInput",
    ),
    ShapeType::Structure,
    &[],
);

/// One value handed to the validating walk, written the way the codec writes
/// that shape so the walk builds the same path and runs the same checks.
pub enum ConstrainedValue<'a> {
    /// A string leaf.
    Text(&'a str),
    /// A list of strings; the walk names each element `<member>.<n>.member`.
    Texts(&'a [String]),
    /// A `String -> String` map; the walk names each value `<member>.<key>.member`.
    Entries(&'a HashMap<String, String>),
    /// A nested structure or union that writes its own leaves.
    Nested(&'a dyn SerializableStruct),
}

/// The members of ONE request whose constraints their operation handed over,
/// written for [`::aws_smithy_schema_server::constraints::validate`].
pub struct ConstrainedMembers<'a> {
    members: &'a [(&'static Schema, ConstrainedValue<'a>)],
}

impl SerializableStruct for ConstrainedMembers<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        for (schema, value) in self.members {
            write_constrained(s, schema, value)?;
        }
        Ok(())
    }
}

/// Write one member for the walk. Mirrors what `vendor/smithy-macros` emits for
/// each shape kind, including passing the collection member schema for a scalar
/// element or map value — which is how the walk tells a collection's entry-count
/// bound from its elements' own bounds.
fn write_constrained(
    s: &mut dyn ShapeSerializer,
    schema: &'static Schema,
    value: &ConstrainedValue<'_>,
) -> Result<(), SerdeError> {
    match value {
        ConstrainedValue::Text(text) => s.write_string(schema, text),
        ConstrainedValue::Texts(texts) => s.write_list(schema, &|sl| {
            for text in texts.iter() {
                sl.write_string(schema, text)?;
            }
            Ok(())
        }),
        ConstrainedValue::Entries(entries) => s.write_map(schema, &|sm| {
            for (key, entry) in entries.iter() {
                sm.write_string(schema, key)?;
                sm.write_string(schema, entry)?;
            }
            Ok(())
        }),
        ConstrainedValue::Nested(nested) => s.write_struct(schema, *nested),
    }
}

/// Run the framework's own constraint walk over the members an operation handed
/// over, and report every violation it found.
pub fn validate_members(
    members: &[(&'static Schema, ConstrainedValue<'_>)],
) -> Vec<Violation> {
    validate(&ConstrainedMembers { members }, &CONSTRAINED_INPUT_SCHEMA)
}

/// A request's `Tags` map, ready for the walk: the map itself — its entry count
/// and each VALUE's own constraints — plus every KEY it carries.
///
/// A map's key shape is constrained just like its value shape, so both sides of
/// an entry are checked before anything is stored. The walk reaches a value
/// through the map member's element constraints but never a key (see
/// [`M_TAGS`]), so each key is handed over as its own leaf, carrying
/// `TagKey`'s constraints through [`M_TAGS_KEY`]. Every operation that accepts
/// `Tags` hands the member over this way, so a key one operation refuses is
/// refused by all of them — and no key a later `UntagResource` could not name is
/// ever written.
pub fn constrained_tags(
    tags: &HashMap<String, String>,
) -> Vec<(&'static Schema, ConstrainedValue<'_>)> {
    let mut members: Vec<(&'static Schema, ConstrainedValue<'_>)> =
        Vec::with_capacity(tags.len() + 1);
    members.push((&M_TAGS, ConstrainedValue::Entries(tags)));
    members.extend(
        tags.keys()
            .map(|key| (&M_TAGS_KEY, ConstrainedValue::Text(key.as_str()))),
    );
    members
}

/// `Code`'s `S3Location` variant, for the walk.
pub struct ConstrainedS3Location<'a> {
    /// `S3Location$Bucket`.
    pub bucket: &'a str,
    /// `S3Location$ObjectKey`.
    pub object_key: &'a str,
    /// `S3Location$VersionId`, when the request carries one.
    pub version_id: Option<&'a str>,
}

impl SerializableStruct for ConstrainedS3Location<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_BUCKET, self.bucket)?;
        s.write_string(&M_OBJECT_KEY, self.object_key)?;
        if let Some(version_id) = self.version_id {
            s.write_string(&M_S3_VERSION_ID, version_id)?;
        }
        Ok(())
    }
}

/// The `Code` union, for the walk.
pub struct ConstrainedCode<'a> {
    /// The union's one modelled variant.
    pub s3_location: ConstrainedS3Location<'a>,
}

impl SerializableStruct for ConstrainedCode<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_struct(&M_S3_LOCATION, &self.s3_location)
    }
}

/// The `Code` union of a request, ready for the walk, or `None` when the
/// request carries none.
pub fn constrained_code(code: Option<&Code>) -> Option<ConstrainedCode<'_>> {
    let Code::S3Location(location) = code?;
    Some(ConstrainedCode {
        s3_location: ConstrainedS3Location {
            bucket: &location.bucket,
            object_key: &location.object_key,
            version_id: location.version_id.as_deref(),
        },
    })
}

/// `EncryptionConfiguration`, for the walk. `KmsKeyId` targets a plain `String`
/// and declares no constraint, so it is not written.
pub struct ConstrainedEncryptionConfiguration<'a> {
    /// `EncryptionConfiguration$Type`.
    pub r#type: &'a str,
}

impl SerializableStruct for ConstrainedEncryptionConfiguration<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_ENCRYPTION_TYPE, self.r#type)
    }
}

/// A request's `EncryptionConfiguration`, ready for the walk.
pub fn constrained_encryption_configuration(
    config: Option<&EncryptionConfiguration>,
) -> Option<ConstrainedEncryptionConfiguration<'_>> {
    config.map(|config| ConstrainedEncryptionConfiguration {
        r#type: &config.r#type,
    })
}

/// `NetworkConfiguration`, for the walk.
pub struct ConstrainedNetworkConfiguration<'a> {
    /// `NetworkConfiguration$SecurityGroupIds`.
    pub security_group_ids: Option<&'a [String]>,
    /// `NetworkConfiguration$SubnetIds`.
    pub subnet_ids: Option<&'a [String]>,
}

impl SerializableStruct for ConstrainedNetworkConfiguration<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        if let Some(ids) = self.security_group_ids {
            write_constrained(s, &M_SECURITY_GROUP_IDS, &ConstrainedValue::Texts(ids))?;
        }
        if let Some(ids) = self.subnet_ids {
            write_constrained(s, &M_SUBNET_IDS, &ConstrainedValue::Texts(ids))?;
        }
        Ok(())
    }
}

/// A request's `NetworkConfiguration`, ready for the walk.
pub fn constrained_network_configuration(
    config: Option<&NetworkConfiguration>,
) -> Option<ConstrainedNetworkConfiguration<'_>> {
    config.map(|config| ConstrainedNetworkConfiguration {
        security_group_ids: config.security_group_ids.as_deref(),
        subnet_ids: config.subnet_ids.as_deref(),
    })
}

// ---------------------------------------------------------------------------
// The service's own ARN validation
// ---------------------------------------------------------------------------
//
// An ARN-typed input member is not reported as a member-constraint failure.
// This service parses the ARNs a request carries with its OWN validators,
// before any member constraint is rendered, and each validator reports its own
// result:
//
// * a resource ARN of this service (a member targeting `WorkflowArn` or
//   `TaggableResourceArn`) is reported under the generic field name `Arn` with
//   the validator's short message, and the top-level `Message` is composed as
//   the service-level prefix plus that field message — so a client attributes
//   an ARN failure to one field name whichever operation refused it;
// * the IAM role ARN (a member targeting `RoleARN`) is reported under the name
//   the model gives the member, with a message built from the request: the
//   fixed prefix plus the rejected value as supplied, which is also the
//   top-level `Message`.
//
// Neither carries the modelled `@pattern` source, which is internal.

/// `FieldList$Name` for a resource ARN this service refused: the generic field
/// name its ARN validator reports, not the member the value arrived in.
const ARN_FIELD_NAME: &str = "Arn";

/// The field message a malformed resource ARN carries.
const INVALID_ARN_MESSAGE: &str = "Invalid arn";

/// How a service-level validation failure composes its top-level `Message`:
/// this fixed prefix followed by the offending field's own message.
const VALIDATION_FAILED_PREFIX: &str = "Validation failed. ";

/// The fixed prefix of the role-ARN validator's message, followed by the
/// rejected value as supplied.
const INVALID_ROLE_ARN_PREFIX: &str = "Invalid Role ARN: ";

/// Which of this service's ARN validators owns an input member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnValidator {
    /// A member naming a resource OF THIS SERVICE (`WorkflowArn`,
    /// `TaggableResourceArn`).
    Resource,
    /// A member naming an IAM role (`RoleARN`).
    Role,
}

/// Every ARN-typed input member of this service, with the validator that owns
/// it — keyed by the member SCHEMA the walk reads, so this list cannot drift
/// from the member names the violation paths carry.
static ARN_MEMBERS: &[(&Schema, ArnValidator)] = &[
    (&M_WORKFLOW_ARN, ArnValidator::Resource),
    (&M_RESOURCE_ARN, ArnValidator::Resource),
    (&M_ROLE_ARN, ArnValidator::Role),
];

/// The ARN validator that owns the member a violation `path` names, or `None`
/// when the path is not one of this service's ARN members. A path is compared
/// against the framework's own lowerCamel member spelling, the exact form it
/// builds a path segment with.
fn arn_validator_for(path: &str) -> Option<ArnValidator> {
    ARN_MEMBERS.iter().find_map(|(schema, validator)| {
        let member = schema.member_name()?;
        (path == ::aws_smithy_schema_server::constraints::path::member(member))
            .then_some(*validator)
    })
}

/// The ARN validator's own `ValidationException` when a request carried a
/// malformed ARN, or `None` when no ARN member was at fault.
///
/// The validators run before member-constraint rendering, so an ARN failure is
/// answered on its own: the `FieldList` carries the ARN field alone, and the
/// `Message` is the one that validator states. A resource ARN names the
/// resource a call targets, so it is parsed before the role ARN a create or
/// update also carries.
fn arn_validation_exception(violations: &[Violation]) -> Option<ValidationException> {
    for owner in [ArnValidator::Resource, ArnValidator::Role] {
        for violation in violations {
            // An ABSENT member is not a malformed ARN: there is no value to
            // parse, so it keeps the missing-member refusal the walk states.
            let Some(value) = violation.value.as_deref() else {
                continue;
            };
            if arn_validator_for(&violation.path) != Some(owner) {
                continue;
            }
            let field = match owner {
                ArnValidator::Resource => ValidationExceptionField {
                    name: ARN_FIELD_NAME.to_string(),
                    message: INVALID_ARN_MESSAGE.to_string(),
                },
                ArnValidator::Role => ValidationExceptionField {
                    name: model_member_name(&violation.path),
                    message: format!("{INVALID_ROLE_ARN_PREFIX}{value}"),
                },
            };
            let message = match owner {
                // The service-level field validation prefixes its field
                // message; the role-ARN check states its message directly.
                ArnValidator::Resource => format!("{VALIDATION_FAILED_PREFIX}{}", field.message),
                ArnValidator::Role => field.message.clone(),
            };
            return Some(ValidationException {
                message,
                reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
                field_list: Some(vec![field]),
            });
        }
    }
    None
}

/// This service's modelled `ValidationException` for the violations the walk
/// reported.
///
/// An ARN member at fault is answered by [`arn_validation_exception`] — this
/// service validates the ARNs a request carries itself, and its validators'
/// results are not the generic constraint rendering.
///
/// Otherwise the `Message` is the framework's own envelope, so a refusal made
/// here reads exactly like one the boundary made; `Reason` is the `@required`
/// `fieldValidationFailed` the boundary cannot carry; and `FieldList` names
/// every member that failed.
///
/// A violation `path` is the walk's lowerCamel rendering (`name`,
/// `subnetIds.0.member`), which is the form AWS validation messages use, so it
/// is kept verbatim in the clause. `FieldList$Name` is documented as "the name
/// of the field that failed validation", so its FIRST segment is restored to the
/// spelling the model gives that member by [`model_member_name`].
pub fn constraint_validation_exception(violations: &[Violation]) -> ValidationException {
    if let Some(exception) = arn_validation_exception(violations) {
        return exception;
    }
    ValidationException {
        message: ::aws_smithy_schema_server::envelope(violations)
            .unwrap_or_else(|| "invalid request".to_string()),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(
            violations
                .iter()
                .map(|violation| ValidationExceptionField {
                    name: model_member_name(&violation.path),
                    message: violation.message(),
                })
                .collect(),
        ),
    }
}

/// A violation path whose FIRST segment is spelled as the model spells that
/// member (`roleArn` -> `RoleArn`), which is the inverse of the walk's single
/// first-character lowering and so exact for this service, whose every member
/// name begins with an uppercase letter.
///
/// Only the first segment is restored. A later segment may be a map KEY the
/// CALLER chose, which must be echoed exactly as sent — the same rule the
/// framework states where it builds those paths — and re-casing it would report
/// a tag key the request never carried.
fn model_member_name(path: &str) -> String {
    let (head, rest) = match path.split_once('.') {
        Some((head, rest)) => (head, Some(rest)),
        None => (path, None),
    };
    let mut chars = head.chars();
    let head: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    match rest {
        Some(rest) => format!("{head}.{rest}"),
        None => head,
    }
}

/// The modelled `ValidationException` for a tagging call that would leave a resource
/// carrying more than [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// The ceiling is a service rule rather than a constraint on one member — the request's
/// own `Tags` map is within its modelled entry count; it is the UNION with the tags the
/// resource already carries that is too large — so it is refused with the same shape,
/// reason and `FieldList` as a constraint failure, naming the member that carried the
/// extra keys.
pub fn too_many_tags(member: &str, resource_arn: &str, resulting: usize) -> ValidationException {
    let message = format!(
        "A resource holds at most {MAX_TAGS_PER_RESOURCE} tags; this request would leave \
         {resource_arn} holding {resulting}"
    );
    ValidationException {
        message: message.clone(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// The modelled `ValidationException` for a change asked of a workflow whose
/// teardown has already started, where `action` completes the clause (`cannot be
/// tagged`).
///
/// A workflow in [`WORKFLOW_STATUS_DELETING`] is on its way out and accepts no
/// change ([`Workflow::is_deleting`]), so every operation that would write to it
/// refuses the request and writes nothing. An operation that models
/// `ConflictException` refuses such a write with it
/// ([`tearing_down_conflict`]); one that does not —
/// `TagResource` and `UntagResource` model no conflict
/// (`smithy_query(query="operation", operation="TagResource,UntagResource")`) —
/// refuses it with the error it does model for a request inconsistent with the
/// service's requirements. The resource's state is not a constraint on any member
/// the request carried, so no field is named and the reason is [`REASON_OTHER`].
pub fn tearing_down_resource(resource_arn: &str, action: &str) -> ValidationException {
    ValidationException {
        message: tearing_down_message(resource_arn, action),
        reason: REASON_OTHER.to_string(),
        field_list: None,
    }
}

/// The modelled `ConflictException` form of that SAME refusal, for an operation
/// that models the conflict — `UpdateWorkflow` and `StartWorkflowRun` — where
/// `action` completes the clause (`cannot be updated`).
///
/// "The resource is in a state that prevents the requested operation"
/// (`smithy_query(query="shape", shape="ConflictException")`) is exactly a
/// workflow in [`WORKFLOW_STATUS_DELETING`] asked for a write, so the refusal
/// names the workflow as the resource whose state blocks the call. The handler
/// keeps only what it cannot share: wrapping this in its own error variant.
pub fn tearing_down_conflict(resource_arn: &str, action: &str) -> ConflictException {
    ConflictException {
        message: tearing_down_message(resource_arn, action),
        resource_id: resource_arn.to_string(),
        resource_type: WORKFLOW_RESOURCE_TYPE.to_string(),
    }
}

/// The message every form of the teardown refusal carries: the workflow, the
/// status its teardown put it in, and the clause `action` completes. One home,
/// so a change to the wording reaches both forms at once.
fn tearing_down_message(resource_arn: &str, action: &str) -> String {
    format!("Workflow {resource_arn} is {WORKFLOW_STATUS_DELETING} and {action}")
}

/// The modelled `ValidationException` for a pagination cursor this service did
/// not issue.
///
/// `NextToken` targets a plain `String` and so declares no constraint the
/// boundary could check; the token's validity is a service rule, and it is
/// refused with the same shape, reason and `FieldList` as a constraint failure.
pub fn invalid_page_token(operation: &str) -> ValidationException {
    ValidationException {
        message: format!(
            "1 validation error detected: Value at 'NextToken' failed to satisfy \
             constraint: Member must be a pagination token issued by {operation}"
        ),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: "NextToken".to_string(),
            message: format!(
                "Value at 'NextToken' failed to satisfy constraint: Member must be a \
                 pagination token issued by {operation}"
            ),
        }]),
    }
}

/// What CloudWatch Logs can hold a log group under, read from the SHARED rule rather than
/// restated here: the predicate over a whole name and the one sentence a refusal states.
/// Re-exported rather than wrapped, so this service's own suites read the rule off `state`
/// while the rule itself has exactly one definition in the tree.
pub use ::service_common::cloudwatch_logs::{is_log_group_name, log_group_name_requirement};

/// The wire path of the log group member a refusal is keyed by.
pub const LOG_GROUP_NAME_MEMBER: &str = "LoggingConfiguration.LogGroupName";

/// Refuse a `LoggingConfiguration` naming a log group CloudWatch Logs could not hold.
///
/// The member is an instruction to CREATE and WRITE a log group — it is where this
/// service exports what a run of the workflow produces ([`run_logs`]) — so the names it
/// admits are the names the service that would hold that group admits, and a value
/// outside them names a group no reader could ever reach. `LogGroupName` targets a bare
/// `String` with neither `@length` nor `@pattern`
/// (`smithy_query(query="shape", shape="LoggingConfiguration")`), so the boundary's
/// constraint walk cannot refuse any of it; the refusal carries the same shape, reason and
/// `FieldList` a constraint failure does, naming the member at fault.
///
/// Every operation that ACCEPTS the configuration judges it this way, before anything is
/// written, so a refused request leaves the workflow exactly as it was.
pub fn check_logging_configuration(
    configuration: Option<&LoggingConfiguration>,
) -> Result<(), ValidationException> {
    let Some(name) = configuration.map(|configuration| configuration.log_group_name.as_str())
    else {
        return Ok(());
    };
    if is_log_group_name(name) {
        return Ok(());
    }
    let clause = format!(
        "Value at '{LOG_GROUP_NAME_MEMBER}' failed to satisfy constraint: Member {}",
        log_group_name_requirement()
    );
    Err(ValidationException {
        message: format!("1 validation error detected: {clause}"),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: LOG_GROUP_NAME_MEMBER.to_string(),
            message: clause,
        }]),
    })
}

/// The wire path of the engine-version member a refusal is keyed by.
pub const ENGINE_VERSION_MEMBER: &str = "EngineVersion";

/// The engine versions this service HAS an engine for: the values the member's own shape
/// defines (`smithy_query(query="shape", shape="EngineVersion")` — an `intEnum` whose one
/// member `ONE` is `1`, "Engine version 1 of Amazon Managed Workflows for Apache Airflow
/// Serverless").
pub const ENGINE_VERSION_VALUES: &[i32] = &[1];

/// Refuse an `EngineVersion` the service defines no engine for.
///
/// A member whose shape ENUMERATES the values it may carry admits only those values, and
/// a request naming another one asks for a configuration the service could not run. The
/// framework's constraint walk refuses such a value for every one of this service's
/// STRING enums (their member schemas carry the enum trait, so
/// [`validate_members`] covers them); the `intEnum` the engine version targets reaches a
/// handler as a plain integer, which no declared `@length`, `@pattern` or `@range` judges,
/// so the membership is checked here instead — with the same shape and reason a constraint
/// failure carries, naming the member at fault.
///
/// Every operation that ACCEPTS the member judges it this way, before anything is minted,
/// so a refused request creates and appends nothing.
pub fn check_engine_version(engine_version: Option<i32>) -> Result<(), ValidationException> {
    let Some(version) = engine_version else {
        return Ok(());
    };
    if ENGINE_VERSION_VALUES.contains(&version) {
        return Ok(());
    }
    Err(ValidationException {
        message: INVALID_ENGINE_VERSION_MESSAGE.to_string(),
        reason: REASON_FIELD_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: ENGINE_VERSION_MEMBER.to_string(),
            message: format!(
                "Value at '{ENGINE_VERSION_MEMBER}' failed to satisfy constraint: Member must \
                 be one of the engine versions this service runs"
            ),
        }]),
    })
}

/// The sentence an engine version this service defines no engine for is refused with:
/// fixed text naming nothing of the request.
pub const INVALID_ENGINE_VERSION_MESSAGE: &str = "Invalid engine version";

/// The sentence a create is refused with when the name it asks for is one a stored
/// workflow already holds.
///
/// "You must use unique workflow names within your account": the refusal reports THAT
/// condition, so it is fixed text and interpolates nothing — not the name asked for, not
/// the holder's identity, which the shape's own required `ResourceId`/`ResourceType`
/// carry. One sentence covers every holder, including one whose teardown has started and
/// so still holds the name.
pub const DUPLICATE_WORKFLOW_NAME_MESSAGE: &str = "Workflow with given name already exists";
