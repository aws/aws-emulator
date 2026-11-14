// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-datasync-fmrsservice service.
//!
//! This module owns the DataSync resources the handlers in `src/ops/` share —
//! transfer locations, transfer tasks and task executions — plus the pieces of
//! behaviour that cannot live in any single operation file:
//!
//! * [`ComAmazonawsDatasyncFmrsserviceState::advance_clock`], which materialises the
//!   transitions the clock has already made due (a task leaving `CREATING`, an
//!   execution walking its phases). Every handler calls it before reading or writing,
//!   so a reporting operation reports what the clock already owed and nothing more.
//! * the WORK each phase of a task execution does, and what it established
//!   ([`transfer`]): the objects the two locations hold, the bytes moved between them
//!   and the per-phase outcome every observer of an execution reads.
//! * whether this service can assume a bucket access role a request names
//!   ([`bucket_access_role`]), which every operation that takes one resolves before it
//!   writes anything.
//! * the modelled defaults ([`default_options`]) and the per-execution overlay
//!   ([`overlay_options`]) that `CreateTask`, `StartTaskExecution` and the two
//!   `Describe*` operations must agree on.
//! * the paging and filtering every `List*` operation shares ([`listing`]): the opaque
//!   cursor, the page size and the modelled comparison operators.
//! * the object storage location members a `CreateLocationObjectStorage` and an
//!   `UpdateLocationObjectStorage` have to resolve the same way
//!   ([`DEFAULT_SERVER_PROTOCOL`], [`default_server_port`] and
//!   [`ComAmazonawsDatasyncFmrsserviceState::object_storage_secret`]).
//!
//! Resource ids are minted from monotonic counters held here, so every ARN this
//! service hands out is also stored here and can be resolved again.

pub mod bucket_access_role;
pub mod listing;
pub mod transfer;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CmkSecretConfig, CustomSecretConfig, Ec2Config, FilterRule, InvalidRequestException,
    ManifestConfig, Options, S3Config, TagListEntry, TaskReportConfig, TaskSchedule,
};

/// How long each clock-owed transition takes.
///
/// The project's emulator contract requires a transition the clock owes to fall due
/// 250–750 ms after the call that starts it, so a caller acting immediately after a
/// create or a start still observes the transient state. 300 ms sits inside that band
/// and is used for every span in this service: `CREATING` -> `AVAILABLE` for a task,
/// and each phase of a task execution.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(300);

/// The region segment to use when a request carries no SigV4 credential scope to read
/// one from (an unsigned request to the emulator).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The longest credential-scope region segment this service will carry into an ARN. No
/// published region identifier comes close, and the segment ends up in every ARN the
/// service stores, so a longer one is treated like a scope it could not read at all.
pub const MAX_REGION_LENGTH: usize = 64;

/// The `datasync` ARN namespace, as the Smithy model's `awsService` trait declares it.
const ARN_NAMESPACE: &str = "datasync";

/// The ARN namespace of the service that holds a location's access credentials, as the
/// modelled `SecretArn` pattern spells it
/// (`^(arn:…:secretsmanager:<region>:<account>:secret:.*|)$`).
const SECRETS_MANAGER_ARN_NAMESPACE: &str = "secretsmanager";

/// A `List*` operation's page size when the request declares none.
///
/// `MaxResults` is modelled `0..100` on every one of this service's list operations, so
/// 100 is the largest page the service will ever return and is what an unspecified page
/// size resolves to.
pub const LIST_DEFAULT_PAGE_SIZE: usize = 100;

/// A RESOURCE GUARD, not an AWS-published quota: every create call appends one record to
/// one of this service's stores, which live in memory for the lifetime of the process and
/// never expire (only a `Delete*` call removes anything), so each store needs a ceiling of its
/// own. A create that would take a store past its ceiling is refused with the modelled
/// client error — see [`ResourceStore`] and
/// [`ComAmazonawsDatasyncFmrsserviceState::at_capacity`] — rather than growing the map.
pub const MAX_LOCATIONS: usize = 10_000;

/// The ceiling on stored transfer tasks. See [`MAX_LOCATIONS`].
pub const MAX_TASKS: usize = 10_000;

/// The ceiling on stored task executions. See [`MAX_LOCATIONS`]. It also bounds the
/// `execution_arns` a single task remembers, because every ARN in that list names an
/// execution this store holds.
pub const MAX_TASK_EXECUTIONS: usize = 10_000;

/// One of the per-request stores a create call grows, for asking whether it is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceStore {
    Locations,
    Tasks,
    TaskExecutions,
}

impl ResourceStore {
    /// How many records this store holds at most.
    const fn ceiling(self) -> usize {
        match self {
            Self::Locations => MAX_LOCATIONS,
            Self::Tasks => MAX_TASKS,
            Self::TaskExecutions => MAX_TASK_EXECUTIONS,
        }
    }

    /// How a refusal names what this store holds.
    const fn resources(self) -> &'static str {
        match self {
            Self::Locations => "transfer locations",
            Self::Tasks => "transfer tasks",
            Self::TaskExecutions => "task executions",
        }
    }
}

/// What kind of storage a transfer location addresses.
///
/// Only the kinds an implemented `CreateLocation*` can mint are listed; the still
/// scaffolded ones will each add their own variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationKind {
    /// An Amazon S3 bucket or access point.
    S3,
    /// A Microsoft Azure Blob Storage container.
    AzureBlob,
    /// A self-managed or third-party object storage system.
    ObjectStorage,
    /// An Amazon EFS file system.
    Efs,
}

impl LocationKind {
    /// The scheme this kind uses in a `LocationUri`.
    fn uri_scheme(self) -> &'static str {
        match self {
            Self::S3 => "s3",
            Self::AzureBlob => "azure-blob",
            Self::ObjectStorage => "object-storage",
            Self::Efs => "efs",
        }
    }

    /// The `LocationUri` for a location of this kind that addresses `global_id` and is
    /// limited to `subdirectory`.
    ///
    /// `LocationUri` is documented as `TYPE://GLOBAL_ID/SUBDIR`, where TYPE is the kind
    /// of location and GLOBAL_ID identifies the resource backing it (a bucket name for
    /// Amazon S3, a container's host and name for Azure Blob Storage, a server and a
    /// bucket for an object storage system, a Region and file system id for Amazon
    /// EFS) — so every kind of location assembles its
    /// URI the same way and contributes only its own GLOBAL_ID. The subdirectory becomes
    /// one slash-delimited prefix, however many leading or trailing slashes the request
    /// spelled it with.
    pub fn location_uri(self, global_id: &str, subdirectory: Option<&str>) -> String {
        let scheme = self.uri_scheme();
        let global_id = global_id.trim_matches('/');
        let prefix = subdirectory.unwrap_or_default().trim_matches('/');
        if prefix.is_empty() {
            format!("{scheme}://{global_id}/")
        } else {
            format!("{scheme}://{global_id}/{prefix}/")
        }
    }

    /// How the modelled `LocationType` filter names this kind of location.
    ///
    /// The filter compares the kind of storage a location addresses — the shape's own
    /// documentation filters Amazon S3 locations with `"Values": "S3"` — so each kind
    /// contributes the upper-case name of its storage. A comparison normalises the
    /// separators in a multi-word name away, so a request spelling one with a hyphen,
    /// an underscore or nothing at all names the same kind.
    pub fn type_name(self) -> &'static str {
        match self {
            Self::S3 => "S3",
            Self::AzureBlob => "AZURE_BLOB",
            Self::ObjectStorage => "OBJECT_STORAGE",
            Self::Efs => "EFS",
        }
    }

    /// How a refusal names this kind of location, article included.
    pub fn described(self) -> &'static str {
        match self {
            Self::S3 => "an Amazon S3",
            Self::AzureBlob => "an Azure Blob Storage",
            Self::ObjectStorage => "an object storage",
            Self::Efs => "an Amazon EFS",
        }
    }
}

/// Where a location's access credential is kept.
///
/// DataSync never reports the credential itself — only the Secrets Manager secret that
/// holds it — so the token a request carries is not part of a location's stored record.
#[derive(Debug, Clone)]
pub enum LocationSecret {
    /// This location needs no stored credential.
    None,
    /// A DataSync-managed secret, encrypted with the Amazon Web Services managed key.
    Managed { secret_arn: String },
    /// A DataSync-managed secret, encrypted with the customer-managed KMS key the
    /// request named.
    ManagedWithCustomerKey {
        secret_arn: String,
        kms_key_arn: Option<String>,
    },
    /// A secret the customer manages, reached through the IAM role the request named.
    Customer {
        secret_arn: Option<String>,
        secret_access_role_arn: Option<String>,
    },
}

impl LocationSecret {
    /// The ARN of the secret this credential lives in, when there is one.
    ///
    /// An update that names only a customer-managed KMS key re-encrypts the secret the
    /// location ALREADY has, so it needs to ask where that secret is.
    pub fn secret_arn(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Managed { secret_arn } | Self::ManagedWithCustomerKey { secret_arn, .. } => {
                Some(secret_arn)
            }
            Self::Customer { secret_arn, .. } => secret_arn.as_deref(),
        }
    }
}

/// The Amazon S3 configuration of a transfer location.
#[derive(Debug, Clone)]
pub struct S3LocationConfig {
    pub bucket_arn: String,
    pub s3_config: S3Config,
    pub storage_class: String,
}

/// The Microsoft Azure Blob Storage configuration of a transfer location.
#[derive(Debug, Clone)]
pub struct AzureBlobLocationConfig {
    pub container_url: String,
    pub authentication_type: String,
    pub blob_type: String,
    pub access_tier: String,
    pub secret: LocationSecret,
}

/// The object storage configuration of a transfer location.
#[derive(Debug, Clone)]
pub struct ObjectStorageLocationConfig {
    pub bucket_name: String,
    pub server_hostname: String,
    pub server_port: i32,
    pub server_protocol: String,
    pub access_key: Option<String>,
    pub server_certificate: Option<Vec<u8>>,
    pub secret: LocationSecret,
}

/// The Amazon EFS configuration of a transfer location.
///
/// `access_point_arn`, `file_system_access_role_arn` and `in_transit_encryption` are
/// each optional in the create request and in the describe response, so a location is
/// stored with exactly what the request named and reports no value the caller never
/// asked for.
#[derive(Debug, Clone)]
pub struct EfsLocationConfig {
    pub filesystem_arn: String,
    pub ec2_config: Ec2Config,
    pub access_point_arn: Option<String>,
    pub file_system_access_role_arn: Option<String>,
    pub in_transit_encryption: Option<String>,
}

/// The per-kind configuration of a transfer location: one variant per
/// [`LocationKind`], so a location holds the members of its own kind and no others.
#[derive(Debug, Clone)]
pub enum LocationConfig {
    S3(S3LocationConfig),
    AzureBlob(AzureBlobLocationConfig),
    ObjectStorage(ObjectStorageLocationConfig),
    Efs(EfsLocationConfig),
}

impl LocationConfig {
    /// The kind of storage this configuration addresses.
    pub fn kind(&self) -> LocationKind {
        match self {
            Self::S3(_) => LocationKind::S3,
            Self::AzureBlob(_) => LocationKind::AzureBlob,
            Self::ObjectStorage(_) => LocationKind::ObjectStorage,
            Self::Efs(_) => LocationKind::Efs,
        }
    }

    /// The Amazon S3 configuration, when that is this location's kind.
    pub fn as_s3(&self) -> Option<&S3LocationConfig> {
        match self {
            Self::S3(s3) => Some(s3),
            _ => None,
        }
    }

    /// The Azure Blob Storage configuration, when that is this location's kind.
    pub fn as_azure_blob(&self) -> Option<&AzureBlobLocationConfig> {
        match self {
            Self::AzureBlob(azure_blob) => Some(azure_blob),
            _ => None,
        }
    }

    /// The object storage configuration, when that is this location's kind.
    pub fn as_object_storage(&self) -> Option<&ObjectStorageLocationConfig> {
        match self {
            Self::ObjectStorage(object_storage) => Some(object_storage),
            _ => None,
        }
    }

    /// The Amazon EFS configuration, when that is this location's kind.
    pub fn as_efs(&self) -> Option<&EfsLocationConfig> {
        match self {
            Self::Efs(efs) => Some(efs),
            _ => None,
        }
    }

    /// The object storage configuration for writing, when that is this location's
    /// kind — what `UpdateLocationObjectStorage` changes.
    pub fn as_object_storage_mut(&mut self) -> Option<&mut ObjectStorageLocationConfig> {
        match self {
            Self::ObjectStorage(object_storage) => Some(object_storage),
            _ => None,
        }
    }
}

/// A transfer location: what a `CreateLocation*` stored and the matching
/// `DescribeLocation*` reports.
///
/// The members every kind of location has live here; the rest are in [`LocationConfig`].
#[derive(Debug, Clone)]
pub struct Location {
    pub arn: String,
    /// The creation sequence this location was minted with — the order `ListLocations`
    /// pages over, and what its opaque cursor resumes after.
    pub seq: u64,
    pub location_uri: String,
    pub subdirectory: Option<String>,
    pub agent_arns: Vec<String>,
    pub tags: Vec<TagListEntry>,
    pub creation_time: SystemTime,
    pub config: LocationConfig,
}

/// A transfer task's lifecycle status, as the modelled `TaskStatus` enum spells it.
///
/// Only the three states this slice can reach are modelled here; `QUEUED` and
/// `UNAVAILABLE` belong to behaviour (task queueing, agent health) that no implemented
/// operation drives yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Creating,
    Available,
    Running,
}

impl TaskStatus {
    /// The wire value, one of `shared_types::TASK_STATUS_VALUES`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Running => "RUNNING",
        }
    }
}

/// A transfer task: what `CreateTask` stored and `DescribeTask`/`ListTasks` report.
#[derive(Debug, Clone)]
pub struct Task {
    pub arn: String,
    /// Creation order. `ListTasks` pages over tasks sorted by this, and its opaque
    /// `NextToken` carries the sequence number the next page resumes after, so a page
    /// boundary survives a task being deleted between pages.
    pub seq: u64,
    pub name: Option<String>,
    pub status: TaskStatus,
    /// When `CREATING` -> `AVAILABLE` falls due. See [`TRANSITION_SPAN`].
    pub available_at: SystemTime,
    pub source_location_arn: String,
    pub destination_location_arn: String,
    pub cloud_watch_log_group_arn: Option<String>,
    pub options: Options,
    pub includes: Vec<FilterRule>,
    pub excludes: Vec<FilterRule>,
    pub schedule: Option<TaskSchedule>,
    pub manifest_config: Option<ManifestConfig>,
    pub task_report_config: Option<TaskReportConfig>,
    pub task_mode: String,
    pub tags: Vec<TagListEntry>,
    pub creation_time: SystemTime,
    pub current_task_execution_arn: Option<String>,
    /// Execution ARNs in creation order, for `ListTaskExecutions` later.
    pub execution_arns: Vec<String>,
}

/// A task execution's status, as the modelled `TaskExecutionStatus` enum spells it.
///
/// `QUEUED` and `CANCELLING` belong to task queueing and `CancelTaskExecution`, neither
/// of which an implemented operation drives yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskExecutionStatus {
    Launching,
    Preparing,
    Transferring,
    Verifying,
    Success,
    /// A phase of this execution failed, so the execution ended without finishing the
    /// work: the phase that failed reports its own `ERROR` in the result detail.
    Error,
}

impl TaskExecutionStatus {
    /// The wire value, one of `shared_types::TASK_EXECUTION_STATUS_VALUES`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launching => "LAUNCHING",
            Self::Preparing => "PREPARING",
            Self::Transferring => "TRANSFERRING",
            Self::Verifying => "VERIFYING",
            Self::Success => "SUCCESS",
            Self::Error => "ERROR",
        }
    }

    /// Whether no further transition is owed from this status.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Success | Self::Error)
    }

    /// This status's position in the phase sequence an execution walks, so a reader
    /// can ask whether a given phase is already behind the execution. Both terminal
    /// statuses sit past every phase.
    pub fn phase_index(self) -> u8 {
        match self {
            Self::Launching => 0,
            Self::Preparing => 1,
            Self::Transferring => 2,
            Self::Verifying => 3,
            Self::Success | Self::Error => 4,
        }
    }

    /// The status an execution whose clock sits at `index` phases is in.
    fn at_index(index: u8) -> Self {
        match index {
            0 => Self::Launching,
            1 => Self::Preparing,
            2 => Self::Transferring,
            3 => Self::Verifying,
            _ => Self::Success,
        }
    }
}

/// One phase of a task execution's status, as the modelled `PhaseStatus` enum spells
/// it.
///
/// A phase is `PENDING` until it has RUN — the clock has not reached the end of its
/// span yet, or an earlier phase failed and this one never starts — and then reports
/// what its work established.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PhaseStatus {
    #[default]
    Pending,
    Success,
    Error,
}

impl PhaseStatus {
    /// The wire value, one of the modelled `PhaseStatus` values.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Success => "SUCCESS",
            Self::Error => "ERROR",
        }
    }
}

/// What the phases of a task execution that have already RUN established.
///
/// The phases are ordered — `PREPARING`, then `TRANSFERRING`, then `VERIFYING` — and
/// each one's work happens when the clock has made its span due
/// ([`transfer::advance_transfers`]). Keeping the outcome here is what lets every
/// observer of an execution read ONE record of what happened rather than deriving an
/// outcome of its own: the per-phase `PhaseStatus` the result detail reports, and the
/// `ErrorCode`/`ErrorDetail` of the phase that failed.
///
/// A phase runs only once every phase BEFORE it is recorded here, so no observer can see
/// a later phase's outcome before an earlier phase has done its work, however many of
/// them are advancing the same execution at the same time.
#[derive(Debug, Clone, Default)]
pub struct ExecutionProgress {
    /// The three phases' statuses, in the order they run.
    pub phases: [PhaseStatus; 3],
    /// `Result.ErrorCode`: the error the failed phase reports.
    pub error_code: Option<String>,
    /// `Result.ErrorDetail`: that error's description.
    pub error_detail: Option<String>,
}

impl ExecutionProgress {
    /// Record `status` as the outcome of the phase at `index`.
    pub fn record(&mut self, index: u8, status: PhaseStatus) {
        if let Some(phase) = self.phases.get_mut(index as usize) {
            *phase = status;
        }
    }

    /// The index of the phase that failed, if one has.
    pub fn failed_phase(&self) -> Option<u8> {
        self.phases
            .iter()
            .position(|phase| *phase == PhaseStatus::Error)
            .map(|index| index as u8)
    }

    /// How many phases, counting from the first, have already run — which is also the
    /// index of the phase that runs next.
    pub fn settled(&self) -> u8 {
        self.phases
            .iter()
            .take_while(|phase| **phase != PhaseStatus::Pending)
            .count() as u8
    }
}

/// The instants at which an execution's phase transitions fall due.
///
/// Stored when the execution starts, so the phase an observer sees is a pure function
/// of the clock and nothing has to sleep or run in the background.
#[derive(Debug, Clone, Copy)]
pub struct ExecutionSchedule {
    /// `LAUNCHING` -> `PREPARING`; also the execution's `LaunchTime`.
    pub launched_at: SystemTime,
    /// `PREPARING` -> `TRANSFERRING`.
    pub transfer_at: SystemTime,
    /// `TRANSFERRING` -> `VERIFYING`.
    pub verify_at: SystemTime,
    /// `VERIFYING` -> `SUCCESS`; also the execution's `EndTime`.
    pub finished_at: SystemTime,
}

impl ExecutionSchedule {
    /// The schedule for an execution started at `start`, one [`TRANSITION_SPAN`] per phase.
    pub fn starting_at(start: SystemTime) -> Self {
        Self {
            launched_at: start + TRANSITION_SPAN,
            transfer_at: start + 2 * TRANSITION_SPAN,
            verify_at: start + 3 * TRANSITION_SPAN,
            finished_at: start + 4 * TRANSITION_SPAN,
        }
    }
}

/// The file and byte counters `DescribeTaskExecution` reports.
///
/// Every one of them is what the phases of this execution actually DID — the objects
/// the `PREPARING` step found on each side, the bytes the `TRANSFERRING` step moved,
/// the objects the `VERIFYING` step compared ([`transfer`]) — so a counter is zero only
/// where no work of that kind happened.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExecutionCounters {
    pub estimated_files_to_transfer: i64,
    pub estimated_bytes_to_transfer: i64,
    pub files_transferred: i64,
    pub bytes_written: i64,
    pub bytes_transferred: i64,
    pub bytes_compressed: i64,
    pub files_deleted: i64,
    pub files_skipped: i64,
    pub files_verified: i64,
    pub estimated_files_to_delete: i64,
    pub files_prepared: i64,
}

/// A task execution: what `StartTaskExecution` stored and `DescribeTaskExecution` reports.
#[derive(Debug, Clone)]
pub struct TaskExecution {
    pub arn: String,
    /// The creation sequence this execution was minted with — the order
    /// `ListTaskExecutions` pages over, and what its opaque cursor resumes after.
    pub seq: u64,
    pub task_arn: String,
    pub status: TaskExecutionStatus,
    /// The options actually in effect: the task's options overlaid with this
    /// execution's `OverrideOptions`.
    pub options: Options,
    pub includes: Vec<FilterRule>,
    pub excludes: Vec<FilterRule>,
    pub manifest_config: Option<ManifestConfig>,
    pub task_report_config: Option<TaskReportConfig>,
    pub tags: Vec<TagListEntry>,
    pub task_mode: String,
    pub start_time: SystemTime,
    pub launch_time: Option<SystemTime>,
    pub end_time: Option<SystemTime>,
    pub schedule: ExecutionSchedule,
    pub counters: ExecutionCounters,
    /// What the phases that have already run established. See [`ExecutionProgress`].
    pub progress: ExecutionProgress,
}

impl TaskExecution {
    /// The status this execution is in at `now`.
    ///
    /// Two things decide it, and both are already recorded: the schedule stored when the
    /// execution started says how far the CLOCK has carried it, and
    /// [`ExecutionProgress`] says what the phases whose work has run established. The
    /// execution never reports a phase as behind it until that phase's work is recorded,
    /// and it ends in `ERROR` once the clock passes the end of a phase that failed.
    pub fn status_at(&self, now: SystemTime) -> TaskExecutionStatus {
        let clock = self.clock_index(now);
        if let Some(failed) = self.progress.failed_phase() {
            // A failed phase ends the execution at the end of its own span; before that
            // instant the execution is still in it.
            if clock > failed + 1 {
                return TaskExecutionStatus::Error;
            }
            return TaskExecutionStatus::at_index(clock.min(failed + 1));
        }
        // A phase whose work has not been materialised yet holds the execution at that
        // phase: nothing reports a step as finished before it ran.
        TaskExecutionStatus::at_index(clock.min(self.progress.settled() + 1))
    }

    /// How many phase boundaries the clock has carried this execution past at `now`: 0
    /// while it is still launching, 4 once its last phase's span has elapsed.
    pub fn clock_index(&self, now: SystemTime) -> u8 {
        if now >= self.schedule.finished_at {
            4
        } else if now >= self.schedule.verify_at {
            3
        } else if now >= self.schedule.transfer_at {
            2
        } else if now >= self.schedule.launched_at {
            1
        } else {
            0
        }
    }

    /// How many of this execution's phases the clock has made DUE at `now` — a phase
    /// falls due at the END of its own span, which is where its work becomes observable.
    pub fn phases_due(&self, now: SystemTime) -> u8 {
        self.clock_index(now).saturating_sub(1).min(3)
    }

    /// When this execution ended, if it has: the end of its last phase, or the end of
    /// the phase that failed.
    pub fn ended_at(&self) -> Option<SystemTime> {
        match self.progress.failed_phase() {
            Some(0) => Some(self.schedule.transfer_at),
            Some(1) => Some(self.schedule.verify_at),
            Some(_) => Some(self.schedule.finished_at),
            None => (self.progress.settled() == 3).then_some(self.schedule.finished_at),
        }
    }
}

/// Per-service state for com-amazonaws-datasync-fmrsservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatasyncFmrsserviceState {
    /// Transfer locations by `LocationArn`.
    pub locations: HashMap<String, Location>,
    /// Transfer tasks by `TaskArn`.
    pub tasks: HashMap<String, Task>,
    /// Task executions by `TaskExecutionArn`.
    pub task_executions: HashMap<String, TaskExecution>,
    next_location_seq: u64,
    next_task_seq: u64,
    next_execution_seq: u64,
    next_secret_seq: u64,
}

impl ComAmazonawsDatasyncFmrsserviceState {
    /// Mint the next `LocationArn` and its creation sequence:
    /// `location/loc-<17 lowercase alphanumerics>`.
    pub fn mint_location_arn(&mut self, region: &str) -> (String, u64) {
        self.next_location_seq += 1;
        let seq = self.next_location_seq;
        (
            emulator_arn(region, &format!("location/loc-{seq:017x}")),
            seq,
        )
    }

    /// Mint the next `TaskArn` and its creation sequence: `task/task-<17 hex digits>`.
    pub fn mint_task_arn(&mut self, region: &str) -> (String, u64) {
        self.next_task_seq += 1;
        let seq = self.next_task_seq;
        (emulator_arn(region, &format!("task/task-{seq:017x}")), seq)
    }

    /// Mint the ARN of the DataSync-managed Secrets Manager secret that holds a
    /// location's access credential: `secret:datasync-<17 hex digits>`.
    pub fn mint_managed_secret_arn(&mut self, region: &str) -> String {
        self.next_secret_seq += 1;
        ::engine::emulator_arn(
            SECRETS_MANAGER_ARN_NAMESPACE,
            region,
            &format!("secret:datasync-{seq:017x}", seq = self.next_secret_seq),
        )
    }

    /// Mint the next `TaskExecutionArn`, which the model nests under its task's ARN, and
    /// its creation sequence — the order `ListTaskExecutions` pages over.
    pub fn mint_execution_arn(&mut self, task_arn: &str) -> (String, u64) {
        self.next_execution_seq += 1;
        let seq = self.next_execution_seq;
        (
            format!("{task_arn}/execution/exec-{seq:017x}"),
            seq,
        )
    }

    /// Materialise every transition the clock has already made due at `now`.
    ///
    /// This is the ONLY write a reporting operation performs: each change here was
    /// owed at `now` whether or not anybody called, because its due instant was stored
    /// when the task was created or the execution was started.
    pub fn advance_clock(&mut self, now: SystemTime) {
        for task in self.tasks.values_mut() {
            if task.status == TaskStatus::Creating && now >= task.available_at {
                task.status = TaskStatus::Available;
            }
        }

        // The executions that have just reached a terminal phase, with the task each
        // belongs to: a task stops being RUNNING when its current execution finishes.
        let mut finished: Vec<(String, String)> = Vec::new();
        for execution in self.task_executions.values_mut() {
            execution.status = execution.status_at(now);
            if execution.launch_time.is_none() && now >= execution.schedule.launched_at {
                execution.launch_time = Some(execution.schedule.launched_at);
            }
            if execution.status.is_terminal() && execution.end_time.is_none() {
                execution.end_time = execution.ended_at();
                finished.push((execution.task_arn.clone(), execution.arn.clone()));
            }
        }

        for (task_arn, execution_arn) in finished {
            if let Some(task) = self.tasks.get_mut(&task_arn) {
                let is_current = task.current_task_execution_arn.as_deref() == Some(&execution_arn);
                if is_current && task.status == TaskStatus::Running {
                    task.status = TaskStatus::Available;
                }
            }
        }
    }

    /// The refusal a create earns because `store` is full, if it is.
    ///
    /// `Some` is the message the calling operation reports through its own modelled
    /// client error, so every store's ceiling is worded the same way.
    pub fn at_capacity(&self, store: ResourceStore) -> Option<String> {
        let held = match store {
            ResourceStore::Locations => self.locations.len(),
            ResourceStore::Tasks => self.tasks.len(),
            ResourceStore::TaskExecutions => self.task_executions.len(),
        };
        (held >= store.ceiling()).then(|| {
            format!(
                "This account already holds the maximum of {ceiling} {resources}.",
                ceiling = store.ceiling(),
                resources = store.resources()
            )
        })
    }

    /// The Amazon S3 location stored under `arn`, with its S3 configuration.
    pub fn s3_location(
        &self,
        arn: &str,
    ) -> Result<(&Location, &S3LocationConfig), LocationRefusal> {
        self.located(arn, LocationKind::S3, LocationConfig::as_s3)
    }

    /// The Azure Blob Storage location stored under `arn`, with its Azure configuration.
    pub fn azure_blob_location(
        &self,
        arn: &str,
    ) -> Result<(&Location, &AzureBlobLocationConfig), LocationRefusal> {
        self.located(arn, LocationKind::AzureBlob, LocationConfig::as_azure_blob)
    }

    /// The Amazon EFS location stored under `arn`, with its EFS configuration.
    pub fn efs_location(
        &self,
        arn: &str,
    ) -> Result<(&Location, &EfsLocationConfig), LocationRefusal> {
        self.located(arn, LocationKind::Efs, LocationConfig::as_efs)
    }

    /// The object storage location stored under `arn`, with its object storage
    /// configuration.
    pub fn object_storage_location(
        &self,
        arn: &str,
    ) -> Result<(&Location, &ObjectStorageLocationConfig), LocationRefusal> {
        self.located(
            arn,
            LocationKind::ObjectStorage,
            LocationConfig::as_object_storage,
        )
    }

    /// The object storage location stored under `arn`, borrowed for an update to write
    /// to: the members every location has that an update can change, and the object
    /// storage configuration. Refused in the same words as the reading lookup.
    pub fn object_storage_location_mut(
        &mut self,
        arn: &str,
    ) -> Result<ObjectStorageLocationMut<'_>, LocationRefusal> {
        let Location {
            location_uri,
            subdirectory,
            agent_arns,
            config,
            ..
        } = self
            .locations
            .get_mut(arn)
            .ok_or_else(|| LocationRefusal::not_found(arn))?;
        let config = config.as_object_storage_mut().ok_or_else(|| {
            LocationRefusal::kind_mismatch(arn, LocationKind::ObjectStorage)
        })?;
        Ok(ObjectStorageLocationMut {
            location_uri,
            subdirectory,
            agent_arns,
            config,
        })
    }

    /// Where an object storage location's credential ends up, from the credential
    /// members of a create or an update request and the secret it already has.
    ///
    /// `CustomSecretConfig` points at a secret the caller manages. `CmkSecretConfig`
    /// asks for a DataSync-managed secret encrypted with the caller's KMS key: with a
    /// `SecretKey` DataSync creates that secret, and on its own it re-encrypts the
    /// secret the location ALREADY has, which therefore stays where it is. A
    /// `SecretKey` alone goes into a secret DataSync manages with the Amazon Web
    /// Services managed key. A request naming none of the three leaves a created
    /// location without a credential and an updated one with the credential it had.
    pub fn object_storage_secret(
        &mut self,
        region: &str,
        current: &LocationSecret,
        secret_key: Option<&str>,
        cmk_secret_config: Option<&CmkSecretConfig>,
        custom_secret_config: Option<&CustomSecretConfig>,
    ) -> LocationSecret {
        if let Some(custom) = custom_secret_config {
            return LocationSecret::Customer {
                secret_arn: custom.secret_arn.clone(),
                secret_access_role_arn: custom.secret_access_role_arn.clone(),
            };
        }
        if let Some(cmk) = cmk_secret_config {
            let held = current.secret_arn().filter(|_| secret_key.is_none());
            let secret_arn = match held {
                Some(secret_arn) => secret_arn.to_string(),
                None => self.mint_managed_secret_arn(region),
            };
            return LocationSecret::ManagedWithCustomerKey {
                secret_arn,
                kms_key_arn: cmk.kms_key_arn.clone(),
            };
        }
        if secret_key.is_some_and(|key| !key.is_empty()) {
            return LocationSecret::Managed {
                secret_arn: self.mint_managed_secret_arn(region),
            };
        }
        current.clone()
    }

    /// The location stored under `arn`, projected through `pick` — the accessor for the
    /// kind of location the asking operation reports.
    ///
    /// A location ARN addresses exactly one location, of exactly one kind, so an ARN
    /// this service never minted and an ARN naming another kind of location are both
    /// client errors rather than empty answers. `Err` carries the message the refusal
    /// reports, so every `DescribeLocation*` words these two refusals the same way.
    fn located<'a, C>(
        &'a self,
        arn: &str,
        kind: LocationKind,
        pick: fn(&'a LocationConfig) -> Option<&'a C>,
    ) -> Result<(&'a Location, &'a C), LocationRefusal> {
        let location = self
            .locations
            .get(arn)
            .ok_or_else(|| LocationRefusal::not_found(arn))?;
        let config =
            pick(&location.config).ok_or_else(|| LocationRefusal::kind_mismatch(arn, kind))?;
        Ok((location, config))
    }

    /// Remove the location stored under `arn`, as `DeleteLocation` does.
    ///
    /// `Err` is the refusal an ARN naming no location this account holds earns: the
    /// service holds a location only because a create stored it, so a delete of an ARN
    /// that resolves to nothing is a client error and nothing is removed.
    pub fn remove_location(&mut self, arn: &str) -> Result<Location, LocationRefusal> {
        self.locations
            .remove(arn)
            .ok_or_else(|| LocationRefusal::not_found(arn))
    }

    /// Tasks in creation order — the order `ListTasks` pages over.
    pub fn tasks_in_creation_order(&self) -> Vec<&Task> {
        let mut tasks: Vec<&Task> = self.tasks.values().collect();
        tasks.sort_by_key(|task| task.seq);
        tasks
    }

    /// Locations in creation order — the order `ListLocations` pages over.
    pub fn locations_in_creation_order(&self) -> Vec<&Location> {
        let mut locations: Vec<&Location> = self.locations.values().collect();
        locations.sort_by_key(|location| location.seq);
        locations
    }

    /// Task executions in creation order — the order `ListTaskExecutions` pages over.
    pub fn executions_in_creation_order(&self) -> Vec<&TaskExecution> {
        let mut executions: Vec<&TaskExecution> = self.task_executions.values().collect();
        executions.sort_by_key(|execution| execution.seq);
        executions
    }
}

/// An ARN in this service's `datasync` namespace, with the emulator's account id.
fn emulator_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, resource)
}

/// The bare resource id of a DataSync ARN — `loc-…`, `task-…`, `agent-…`.
///
/// This is how DataSync identifies a resource in a refusal message: the id alone,
/// never the ARN the request carried and never the member that carried it.
pub fn resource_id(arn: &str) -> &str {
    arn.split('/').next_back().unwrap_or(arn)
}

/// The region a request was signed for, from its SigV4 credential scope.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a scope whose
/// region segment could not appear in an ARN this service mints (the modelled ARN
/// patterns allow `[a-z\-0-9]+` there) or is longer than any region identifier
/// ([`MAX_REGION_LENGTH`]) — the segment is carried by every ARN this service stores,
/// so an unbounded one would inflate every stored record.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LENGTH
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `SystemTime` as the modelled `Time` timestamp.
pub fn as_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// Whole milliseconds from `from` to `to`, or 0 when `to` precedes `from`.
pub fn millis_between(from: SystemTime, to: SystemTime) -> i64 {
    to.duration_since(from)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The protocol an object storage location whose request named none ends up with:
/// "If not specified, the default value is HTTPS" (`ServerProtocol`'s own
/// documentation).
pub const DEFAULT_SERVER_PROTOCOL: &str = "HTTPS";

/// The port an object storage request that named none ends up with: the one its
/// protocol speaks.
///
/// The modelled `ObjectStorageServerProtocol` has exactly two values, and each has a
/// well-known port — 443 for HTTPS, 80 for HTTP — so an unspecified `ServerPort`
/// resolves to the default port of the protocol the location uses.
pub fn default_server_port(server_protocol: &str) -> i32 {
    if server_protocol == "HTTP" {
        80
    } else {
        443
    }
}

/// The class discriminator DataSync reports for a request that names a location the
/// account does not hold.
///
/// Every client failure of every DataSync operation is the one modelled client error,
/// so the refusal CLASS is carried by the two code members of that error rather than by
/// a distinct exception shape — and a caller branching on "the location is gone" reads
/// it there.
pub const LOCATION_NOT_FOUND_CODE: &str = "LocationNotFound";

/// The class discriminator DataSync reports for a request whose parameters it will not
/// accept — a member outside the values the operation takes, or a dependency between
/// members that the shapes alone cannot state.
///
/// Carried by both code members of the modelled client error, like every other refusal
/// class of this service.
pub const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The class discriminator DataSync reports for a task whose two `LocationArn` members
/// name the SAME location.
///
/// A task copies one location onto another, so the degenerate pair breaks a rule of its
/// own rather than being a bad parameter value: the refusal names that rule, and a caller
/// branching on it reads the discriminator in both code members like every other refusal
/// class of this service.
pub const DEST_LOC_SAME_AS_SRC_LOC_CODE: &str = "DestLocSameAsSrcLoc";

/// Why resolving a `LocationArn` against this service's state refused the request.
///
/// Carries what the modelled client error reports: the message, and the class
/// discriminator when the refusal class has one. One refusal type for every operation
/// that resolves a location, so a read, an update and a delete word and classify a
/// missing location the same way.
#[derive(Debug, Clone)]
pub struct LocationRefusal {
    pub message: String,
    /// The value BOTH `errorCode` and `datasyncErrorCode` carry, when this refusal
    /// class has a published discriminator.
    pub error_code: Option<String>,
}

impl LocationRefusal {
    /// The refusal an ARN that names no location this account holds earns.
    pub fn not_found(location_arn: &str) -> Self {
        Self {
            message: format!("Location {location_arn} is not found."),
            error_code: Some(LOCATION_NOT_FOUND_CODE.to_string()),
        }
    }

    /// The refusal an ARN that names a location of another kind than the one the asking
    /// operation reports on earns.
    pub fn kind_mismatch(location_arn: &str, expected: LocationKind) -> Self {
        Self {
            message: format!(
                "Location {location_arn} is not {described} location.",
                described = expected.described()
            ),
            error_code: None,
        }
    }

    /// This refusal as the modelled client error every location operation reports it
    /// through: one discriminator fills both code members.
    pub fn into_invalid_request(self) -> InvalidRequestException {
        InvalidRequestException {
            message: Some(self.message),
            error_code: self.error_code.clone(),
            datasync_error_code: self.error_code,
        }
    }
}

/// An object storage location borrowed for an update: the members every location has
/// that `UpdateLocationObjectStorage` can change, beside the object storage
/// configuration it changes the rest of.
#[derive(Debug)]
pub struct ObjectStorageLocationMut<'a> {
    pub location_uri: &'a mut String,
    pub subdirectory: &'a mut Option<String>,
    pub agent_arns: &'a mut Vec<String>,
    pub config: &'a mut ObjectStorageLocationConfig,
}

/// The task options a `CreateTask` that declared none ends up with.
///
/// Every value is the default the Smithy model's own member documentation names:
/// `OverwriteMode ALWAYS`, `Atime BEST_EFFORT`, `Mtime PRESERVE`, `Uid INT_VALUE`,
/// `Gid INT_VALUE`, `PreserveDeletedFiles PRESERVE`, `PreserveDevices NONE`,
/// `PosixPermissions PRESERVE`, `TaskQueueing ENABLED`, `TransferMode CHANGED`,
/// `SecurityDescriptorCopyFlags OWNER_DACL`, `ObjectTags PRESERVE`,
/// `ObjectMetadata PRESERVE`. `VerifyMode` is mode-dependent: the model documents
/// `ONLY_FILES_TRANSFERRED` as "the default option for Enhanced mode tasks" and
/// `POINT_IN_TIME_CONSISTENT` as "the default option for Basic mode tasks".
/// `BytesPerSecond` is the no-limit sentinel the member's `range` admits (`min: -1`),
/// and `LogLevel` is `OFF`, the value the model describes as publishing no logs —
/// which is what a task with no `CloudWatchLogGroupArn` does.
pub fn default_options(task_mode: &str) -> Options {
    Options {
        verify_mode: Some(
            if task_mode == "ENHANCED" {
                "ONLY_FILES_TRANSFERRED"
            } else {
                "POINT_IN_TIME_CONSISTENT"
            }
            .to_string(),
        ),
        overwrite_mode: Some("ALWAYS".to_string()),
        atime: Some("BEST_EFFORT".to_string()),
        mtime: Some("PRESERVE".to_string()),
        uid: Some("INT_VALUE".to_string()),
        gid: Some("INT_VALUE".to_string()),
        preserve_deleted_files: Some("PRESERVE".to_string()),
        preserve_devices: Some("NONE".to_string()),
        posix_permissions: Some("PRESERVE".to_string()),
        bytes_per_second: Some(-1),
        task_queueing: Some("ENABLED".to_string()),
        log_level: Some("OFF".to_string()),
        transfer_mode: Some("CHANGED".to_string()),
        security_descriptor_copy_flags: Some("OWNER_DACL".to_string()),
        object_tags: Some("PRESERVE".to_string()),
        object_metadata: Some("PRESERVE".to_string()),
    }
}

/// `base` with every member the caller declared in `requested` replacing it.
///
/// Used twice, for the same reason the model documents once ("Each option has a
/// default value… You also can override your task options for each task execution"):
/// `CreateTask` overlays the request's `Options` on [`default_options`], and
/// `StartTaskExecution` overlays `OverrideOptions` on the task's stored options.
pub fn overlay_options(base: &Options, requested: Option<&Options>) -> Options {
    let Some(requested) = requested else {
        return base.clone();
    };
    Options {
        verify_mode: requested
            .verify_mode
            .clone()
            .or_else(|| base.verify_mode.clone()),
        overwrite_mode: requested
            .overwrite_mode
            .clone()
            .or_else(|| base.overwrite_mode.clone()),
        atime: requested.atime.clone().or_else(|| base.atime.clone()),
        mtime: requested.mtime.clone().or_else(|| base.mtime.clone()),
        uid: requested.uid.clone().or_else(|| base.uid.clone()),
        gid: requested.gid.clone().or_else(|| base.gid.clone()),
        preserve_deleted_files: requested
            .preserve_deleted_files
            .clone()
            .or_else(|| base.preserve_deleted_files.clone()),
        preserve_devices: requested
            .preserve_devices
            .clone()
            .or_else(|| base.preserve_devices.clone()),
        posix_permissions: requested
            .posix_permissions
            .clone()
            .or_else(|| base.posix_permissions.clone()),
        bytes_per_second: requested.bytes_per_second.or(base.bytes_per_second),
        task_queueing: requested
            .task_queueing
            .clone()
            .or_else(|| base.task_queueing.clone()),
        log_level: requested.log_level.clone().or_else(|| base.log_level.clone()),
        transfer_mode: requested
            .transfer_mode
            .clone()
            .or_else(|| base.transfer_mode.clone()),
        security_descriptor_copy_flags: requested
            .security_descriptor_copy_flags
            .clone()
            .or_else(|| base.security_descriptor_copy_flags.clone()),
        object_tags: requested
            .object_tags
            .clone()
            .or_else(|| base.object_tags.clone()),
        object_metadata: requested
            .object_metadata
            .clone()
            .or_else(|| base.object_metadata.clone()),
    }
}

/// The bucket or access-point name an `S3BucketArn` addresses.
///
/// The member's three modelled ARN forms all put the name last in the resource
/// segment: `arn:<p>:s3:::<bucket>`, `arn:<p>:s3:<region>:<account>:accesspoint/<name>`
/// and `arn:<p>:s3-outposts:<region>:<account>:outpost/<outpost>/accesspoint/<name>`.
pub fn s3_bucket_name(bucket_arn: &str) -> String {
    let resource = bucket_arn.splitn(6, ':').nth(5).unwrap_or(bucket_arn);
    resource
        .split(['/', ':'])
        .next_back()
        .unwrap_or(resource)
        .to_string()
}

/// The same member narrowed to the form that names a BUCKET: the bucket name of an
/// `arn:<partition>:s3:::<bucket>`, and `None` for either access-point form, which names a
/// resource of its own rather than a bucket.
///
/// The one reading of that distinction, for a caller that has to ask the object store about
/// the bucket itself ([`transfer::bucket_is_absent`]): a bucket ARN is the form carrying
/// neither a Region nor an account, whose whole resource is the name
/// [`s3_bucket_name`] reports.
pub fn s3_plain_bucket_name(bucket_arn: &str) -> Option<&str> {
    let arn = ::service_common::arn::parse_of(bucket_arn, "s3")?;
    if !arn.region.is_empty() || !arn.account.is_empty() {
        return None;
    }
    (!arn.resource.is_empty() && !arn.resource.contains(['/', ':'])).then_some(arn.resource)
}

/// A pair of `Options` members whose values are each legal on its own and which
/// contradict each other, so a request carrying both is refused.
///
/// The modelled enums admit each value independently; what the pairs here state is the
/// dependency BETWEEN two members that no shape can state, which is why every one of
/// them is grounded in what the service documents or does rather than in the model's
/// enum domains.
struct OptionConflict {
    /// The first member and the value of it that takes part in the conflict.
    first: (&'static str, &'static str),
    /// The second member and its conflicting value.
    second: (&'static str, &'static str),
    /// Whether the dependency applies only to a `BASIC` mode task, as the members whose
    /// documentation scopes it that way ("The following applies only to Basic mode
    /// tasks") require.
    basic_mode_only: bool,
}

/// Every `Options` combination this service refuses.
///
/// * `TransferMode ALL` with `OverwriteMode NEVER`: a transfer that "copies everything
///   in the source to the destination without comparing differences between the
///   locations" cannot at the same time "not overwrite data in the destination", so the
///   two options cancel each other out and DataSync refuses the pair.
/// * `TransferMode ALL` with `PreserveDeletedFiles REMOVE`: "If you set this parameter
///   to REMOVE, you can't set TransferMode to ALL. When you transfer all data, DataSync
///   doesn't scan your destination location and doesn't know what to delete"
///   (`PreserveDeletedFiles`' own documentation).
/// * `Atime BEST_EFFORT` with `Mtime NONE`, and `Atime NONE` with `Mtime PRESERVE`:
///   "If Atime is set to BEST_EFFORT, Mtime must be set to PRESERVE… If Atime is set to
///   NONE, Mtime must also be NONE" (`Atime`'s own documentation), which applies to
///   Basic mode tasks.
const OPTION_CONFLICTS: &[OptionConflict] = &[
    OptionConflict {
        first: ("TransferMode", "ALL"),
        second: ("OverwriteMode", "NEVER"),
        basic_mode_only: false,
    },
    OptionConflict {
        first: ("TransferMode", "ALL"),
        second: ("PreserveDeletedFiles", "REMOVE"),
        basic_mode_only: false,
    },
    OptionConflict {
        first: ("Atime", "BEST_EFFORT"),
        second: ("Mtime", "NONE"),
        basic_mode_only: true,
    },
    OptionConflict {
        first: ("Atime", "NONE"),
        second: ("Mtime", "PRESERVE"),
        basic_mode_only: true,
    },
];

/// The refusal the EFFECTIVE options of a task or of a task execution earn for naming a
/// contradictory combination, if they do.
///
/// `Some` is the message the calling operation reports through its own modelled client
/// error, with the members and their values named the way DataSync names them, so every
/// operation that resolves a set of options words this refusal identically. The options
/// passed in are the resolved ones — the request's values over the defaults — because a
/// combination is contradictory whether the caller spelled both halves of it or took one
/// from a default.
pub fn conflicting_options(options: &Options, task_mode: &str) -> Option<String> {
    let value = |member: &str| -> &str {
        match member {
            "TransferMode" => options.transfer_mode.as_deref(),
            "OverwriteMode" => options.overwrite_mode.as_deref(),
            "PreserveDeletedFiles" => options.preserve_deleted_files.as_deref(),
            "Atime" => options.atime.as_deref(),
            "Mtime" => options.mtime.as_deref(),
            _ => None,
        }
        .unwrap_or_default()
    };
    OPTION_CONFLICTS
        .iter()
        .find(|conflict| {
            (!conflict.basic_mode_only || task_mode != "ENHANCED")
                && value(conflict.first.0) == conflict.first.1
                && value(conflict.second.0) == conflict.second.1
        })
        .map(|conflict| {
            format!(
                "Invalid SyncOption value. Option: {first},{second}, Value: {first_value},{second_value}.",
                first = conflict.first.0,
                second = conflict.second.0,
                first_value = conflict.first.1,
                second_value = conflict.second.1,
            )
        })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatasyncFmrsserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatasyncFmrsserviceState>)
}
