// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-braket-braket service.
//!
//! Everything the Braket operations answer with is computed from the values held
//! here: the seeded device inventory (devices are published by AWS, never created
//! through the API), the quantum-task records `CreateQuantumTask` writes and the
//! hybrid-job records `CreateJob` writes.
//!
//! This module also owns the few helpers more than one operation needs — the
//! request's region, the clock-owed status transitions, request digests and the
//! opaque pagination cursor — because `ops/mod.rs` is generated and cannot carry a
//! shared module.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AlgorithmSpecification, Association, DeviceConfig, DeviceStatusReason, ExperimentalCapabilities,
    InputFileConfig, InstanceConfig, JobCheckpointConfig, JobOutputDataConfig, JobStoppingCondition,
    TimePeriod,
};

/// The program a quantum task's `action` document carries, split out of this root as its
/// own concern; its names are re-exported so every caller keeps reaching them through
/// `crate::state`.
pub mod quantum_program;
pub use quantum_program::*;

/// The simulator that RUNS a lowered gate-model program, split out and re-exported the
/// same way.
pub mod quantum_simulation;
pub use quantum_simulation::*;

/// A finished task's result document, and the Amazon S3 bucket it is written to — which
/// Amazon S3 owns, so it is resolved there; split out and re-exported the same way.
pub mod task_results;
pub use task_results::*;

/// The device-parameters document a quantum task ran under, split out and re-exported the
/// same way.
pub mod device_parameters;
pub use device_parameters::*;

// ---------------------------------------------------------------------------
// Modelled enum values this service stores and emits.
//
// Each one is a member of the matching `*_VALUES` catalog in
// `ops::types::shared_types`, which is generated from the Smithy model; the test
// `modelled_enum_values_are_the_only_ones_the_service_stores` pins that.
// ---------------------------------------------------------------------------

/// `QuantumTaskStatus` — the task is accepted and not queued yet.
pub const STATUS_CREATED: &str = "CREATED";
/// `QuantumTaskStatus` / `JobPrimaryStatus` — the work is waiting for its device or
/// its instances.
pub const STATUS_QUEUED: &str = "QUEUED";
/// `QuantumTaskStatus` / `JobPrimaryStatus` — the work is executing.
pub const STATUS_RUNNING: &str = "RUNNING";
/// `QuantumTaskStatus` / `JobPrimaryStatus` — the work reached a terminal failure.
pub const STATUS_FAILED: &str = "FAILED";
/// `QuantumTaskStatus` / `JobPrimaryStatus` — the work completed successfully.
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// `QuantumTaskStatus` / `JobPrimaryStatus` / `CancellationStatus` — cancellation is in
/// progress.
pub const STATUS_CANCELLING: &str = "CANCELLING";
/// `QuantumTaskStatus` / `JobPrimaryStatus` / `CancellationStatus` — the work was
/// cancelled.
pub const STATUS_CANCELLED: &str = "CANCELLED";

/// `DeviceStatus` values.
pub const DEVICE_ONLINE: &str = "ONLINE";
pub const DEVICE_OFFLINE: &str = "OFFLINE";
/// The device is held out of service for the time being, rather than being unavailable
/// for the moment ([`DEVICE_OFFLINE`]) or gone for good ([`DEVICE_RETIRED`]).
pub const DEVICE_PAUSED: &str = "PAUSED";
pub const DEVICE_RETIRED: &str = "RETIRED";

/// `DeviceStatusReasonCode` — the device is unavailable because the clock is outside the
/// execution windows its capability document publishes.
pub const REASON_OUTSIDE_AVAILABILITY_WINDOW: &str = "OUTSIDE_AVAILABILITY_WINDOW";
/// `DeviceStatusReasonCode` — the device is held out of service while it is worked on.
pub const REASON_MAINTENANCE: &str = "MAINTENANCE";

/// `DeviceType` values.
pub const DEVICE_TYPE_QPU: &str = "QPU";
pub const DEVICE_TYPE_SIMULATOR: &str = "SIMULATOR";

/// The unit a device's published `deviceCost` is charged per: a QPU bills each SHOT a
/// quantum task asks for, a simulator each MINUTE it simulates for.
pub const PRICE_UNIT_SHOT: &str = "shot";
/// See [`PRICE_UNIT_SHOT`].
pub const PRICE_UNIT_MINUTE: &str = "minute";

/// `QueueName` values.
pub const QUEUE_QUANTUM_TASKS: &str = "QUANTUM_TASKS_QUEUE";
pub const QUEUE_JOBS: &str = "JOBS_QUEUE";

/// `QueuePriority` values.
pub const PRIORITY_NORMAL: &str = "Normal";
pub const PRIORITY_PRIORITY: &str = "Priority";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one. `us-east-1` is the AWS default region.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Clock-owed transitions.
//
// A quantum task AWS accepts is returned in the non-terminal `CREATED` status, so
// its later statuses are owed by the clock rather than by any call. The spans are
// the emulator's own choice (AWS's depend on real device queues); they are never
// zero, so a caller acting immediately after `CreateQuantumTask` still observes
// `CREATED` and the state errors that go with it. Nothing sleeps: the due instant
// is stored on the record and materialised by the next call that reads it.
// ---------------------------------------------------------------------------

/// `CREATED` -> `QUEUED`, measured from `CreateQuantumTask`.
pub const QUEUE_SPAN: Duration = Duration::from_millis(600);
/// `QUEUED` -> terminal, measured from `CreateQuantumTask`: 600 ms after the queue
/// transition that starts it.
pub const TERMINAL_SPAN: Duration = Duration::from_millis(1_200);
/// `CANCELLING` -> `CANCELLED`, measured from `CancelQuantumTask`.
pub const CANCELLATION_SPAN: Duration = Duration::from_millis(600);

/// `QUEUED` -> `RUNNING` for a hybrid job, measured from `CreateJob`.
///
/// `CreateJob` returns a job in the non-terminal `QUEUED` status, so its later
/// statuses are owed by the clock. As with the quantum-task spans above, the span is
/// the emulator's own choice and never zero, so a caller reading the job immediately
/// after creating it still observes `QUEUED`.
pub const JOB_RUNNING_SPAN: Duration = Duration::from_millis(600);
/// `RUNNING` -> terminal for a hybrid job, measured from `CreateJob`: 600 ms after
/// the transition that starts it.
pub const JOB_TERMINAL_SPAN: Duration = Duration::from_millis(1_200);

/// Why a quantum task this emulator accepted ends in `FAILED` when its program never
/// ran.
///
/// A task ends in the failure status exactly when there is no result to read: the program
/// could not be lowered to a circuit, the simulator could not apply one of its gates, or
/// the result document could not be written to the output location. The reason a task
/// carries is the one its own run produced ([`QuantumTask::outcome`]), so what a client
/// reads says which of those happened; this is the fallback for a task whose record
/// carries none.
pub const NOT_EXECUTED_FAILURE_REASON: &str =
    "The quantum task produced no result, so no result was written to the output S3 location.";

/// Why a hybrid job this emulator accepted ends in `FAILED`.
///
/// A Braket hybrid job runs a caller-supplied Python entry point on a provisioned ML
/// instance, from a container image or an S3 script bundle. This emulator provisions
/// no instance and runs no entry point, so reporting `COMPLETED` would claim
/// artifacts that were never written; the job ends in the modelled failure status
/// carrying this reason instead.
pub const JOB_NOT_EXECUTED_FAILURE_REASON: &str = "This emulator does not run hybrid job \
algorithms: no instance was provisioned and no entry point was executed, so no job artifacts \
were written to the output S3 location.";

// ---------------------------------------------------------------------------
// Bounds on the caller-driven stores
// ---------------------------------------------------------------------------

/// The most quantum tasks one device may have in flight (not yet terminal) at once.
///
/// Amazon Braket publishes a per-device concurrent-quantum-task quota and answers a
/// request past it with `ServiceQuotaExceededException`; this is the emulator's own
/// figure for every device, and it is what bounds how many live records one caller
/// can hold open.
pub const MAX_TASKS_IN_FLIGHT_PER_DEVICE: usize = 100;

/// The most quantum-task records this emulator keeps at once.
///
/// AWS retains a task's metadata for a documented period; this emulator keeps state
/// in memory for the life of the process, so it bounds the store by COUNT instead: a
/// create makes room first, dropping the oldest FINISHED records (see
/// [`ComAmazonawsBraketBraketState::evict_finished_tasks_over_capacity`]), so the store
/// never holds more than this many — which also bounds the per-request sweep that
/// materialises due transitions. It is far above the in-flight quota above, so a task
/// is only ever dropped after it has finished.
pub const MAX_QUANTUM_TASKS: usize = 1_000;

/// The most `clientToken` entries this emulator remembers at once.
///
/// One per retained resource: a token whose resource has been dropped is of no further
/// use, because the ARN it would replay no longer resolves. With this cap and
/// [`IDEMPOTENCY_TTL`], a caller sending an endless stream of fresh tokens cannot grow
/// the index without limit (see
/// [`ComAmazonawsBraketBraketState::prune_idempotency`]).
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_QUANTUM_TASKS;

/// The most spending-limit records this emulator keeps at once.
///
/// A spending limit has no terminal state to age out of — it lives until
/// `DeleteSpendingLimit` removes it — so the store cannot make room by evicting
/// finished records the way the quantum-task store does. It is bounded by COUNT
/// instead, and a create past this many is refused as invalid input: Amazon Braket
/// publishes a service quota on spending limits, and `CreateSpendingLimit` models no
/// `ServiceQuotaExceededException`, so `ValidationException` is the refusal it has.
pub const MAX_SPENDING_LIMITS: usize = 1_000;

/// The period a spending limit covers when the caller sends no `timePeriod`.
///
/// `timePeriod` is optional on the way IN and required on the way OUT, so a limit
/// created without one still has a period to report. AWS documents no default window,
/// so this span is the emulator's own choice; a `timePeriod` the caller DOES send is
/// stored and reported exactly as sent.
pub const DEFAULT_SPENDING_LIMIT_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long a `clientToken` is remembered for.
///
/// AWS does not document the window Braket honours an idempotency token for, so this
/// span is the emulator's own choice: long enough that a client retrying a create
/// (minutes at most) always gets the first ARN back, short enough that the index does
/// not hold tokens for work that finished a day ago.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The most hybrid jobs that may be in flight (not yet terminal) at once.
///
/// Amazon Braket publishes a quota for concurrent hybrid jobs and answers a request
/// past it with `ServiceQuotaExceededException`; this is the emulator's own figure,
/// and it is what bounds how many live job records one caller can hold open.
pub const MAX_JOBS_IN_FLIGHT: usize = 100;

/// The most hybrid-job records this emulator keeps at once, bounding the job store by
/// COUNT exactly as [`MAX_QUANTUM_TASKS`] bounds the quantum-task store.
pub const MAX_JOBS: usize = 1_000;

/// The most tags one resource may carry.
///
/// AWS's resource tagging limits allow at most 50 tags on a resource, so `TagResource`
/// refuses a call that would take a resource past that figure. It is also what bounds
/// the tag state one caller can accumulate on a stored record, the way the constants
/// above bound how many records the stores hold.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// A device in the seeded inventory. Devices are published by AWS and are not
/// created, updated or deleted through the Braket API.
#[derive(Debug, Clone)]
pub struct Device {
    pub device_arn: String,
    pub device_name: String,
    pub provider_name: String,
    /// One of [`DEVICE_TYPE_QPU`] / [`DEVICE_TYPE_SIMULATOR`].
    pub device_type: String,
    /// One of [`DEVICE_ONLINE`] / [`DEVICE_OFFLINE`] / [`DEVICE_PAUSED`] /
    /// [`DEVICE_RETIRED`].
    pub device_status: String,
    /// Why the device is in that status, for a status the catalog publishes a reason for;
    /// `None` for a status that has none. Built by [`status_reason`] from the row itself,
    /// so the reason a caller reads can never disagree with the status beside it.
    pub device_status_reason: Option<DeviceStatusReason>,
    /// The device capability document, as the JSON string `deviceCapabilities`
    /// carries (`@mediaType("application/json")`).
    pub device_capabilities: String,
    /// The inclusive shot range this device's capability document publishes, as
    /// `service.shotsRange`. `CreateQuantumTask` refuses a `shots` value outside
    /// it, so the catalog a caller reads and the rule it is held to agree.
    pub shots_range: (i64, i64),
    /// What one charged unit of this device costs in USD, as its capability document
    /// publishes it (`service.deviceCost.price`). Spending on a device is computed
    /// from this figure, so the price a caller reads and the spend reported against a
    /// spending limit are the same data.
    pub cost_price: f64,
    /// The unit [`Device::cost_price`] is charged per (`service.deviceCost.unit`):
    /// [`PRICE_UNIT_SHOT`] or [`PRICE_UNIT_MINUTE`].
    pub cost_unit: String,
}

/// What the run of a quantum task produced, decided when the task was created: the
/// program was executed against the device's simulator and its result document written,
/// or it was not, with the reason why.
///
/// A task AWS accepts is returned in a non-terminal status and reaches its terminal one
/// on the clock, so the status a caller reads is still owed by the clock
/// ([`TERMINAL_SPAN`]); this is WHICH terminal status is owed, and it is decided by what
/// actually ran rather than by the status being due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    /// The program ran and its result document was written to the output location: this
    /// many shots produced a measurement, which is the task's `numSuccessfulShots`.
    Completed { successful_shots: i64 },
    /// No result was produced, for the reason given — the task's `failureReason`.
    Failed { reason: String },
}

impl TaskOutcome {
    /// The terminal status this outcome reaches.
    #[must_use]
    pub fn terminal_status(&self) -> &'static str {
        match self {
            Self::Completed { .. } => STATUS_COMPLETED,
            Self::Failed { .. } => STATUS_FAILED,
        }
    }

    /// The shots that produced a measurement, or `None` for a task that produced no
    /// result.
    #[must_use]
    pub fn successful_shots(&self) -> Option<i64> {
        match self {
            Self::Completed { successful_shots } => Some(*successful_shots),
            Self::Failed { .. } => None,
        }
    }
}

/// A quantum task created through `CreateQuantumTask`.
#[derive(Debug, Clone)]
pub struct QuantumTask {
    pub quantum_task_arn: String,
    /// A `QuantumTaskStatus` value.
    pub status: String,
    pub device_arn: String,
    pub device_parameters: String,
    pub action: String,
    pub shots: i64,
    pub output_s3_bucket: String,
    pub output_s3_key_prefix: String,
    pub output_s3_directory: String,
    pub created_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
    /// The ARN of the hybrid job that submitted this task. Stays `None`: a task is
    /// attributed to a job through the `jobToken` the job's own container is handed,
    /// and this emulator runs no job container, so no `jobToken` a caller presents
    /// names a job it minted.
    pub job_arn: Option<String>,
    pub job_token: Option<String>,
    pub associations: Vec<Association>,
    pub tags: BTreeMap<String, String>,
    /// The experimental capabilities the task was created with, if any.
    pub experimental_capabilities: Option<ExperimentalCapabilities>,
    /// When the `CREATED` -> `QUEUED` transition falls due.
    pub queued_due: SystemTime,
    /// When the terminal transition falls due.
    pub terminal_due: SystemTime,
    /// When `CANCELLING` -> `CANCELLED` falls due; set by `CancelQuantumTask`.
    pub cancelled_due: Option<SystemTime>,
    /// What the device's run of this task produced — the terminal status the clock owes
    /// it, and the `numSuccessfulShots` or `failureReason` that goes with it.
    pub outcome: TaskOutcome,
    /// The key of the result object this task's run wrote under
    /// [`QuantumTask::output_s3_directory`], for a task whose run produced one.
    pub result_object_key: Option<String>,
}

impl QuantumTask {
    /// Whether the task is in a status it can never leave.
    pub fn is_terminal(&self) -> bool {
        self.status == STATUS_COMPLETED
            || self.status == STATUS_FAILED
            || self.status == STATUS_CANCELLED
    }

    /// Whether the task still occupies its device's queue.
    pub fn is_queued(&self) -> bool {
        self.status == STATUS_CREATED || self.status == STATUS_QUEUED
    }

    /// The queue priority a task runs at: tasks submitted by a hybrid job run on
    /// the priority queue, every other task on the normal one.
    pub fn queue_priority(&self) -> &'static str {
        if self.job_token.is_some() {
            PRIORITY_PRIORITY
        } else {
            PRIORITY_NORMAL
        }
    }
}

/// A hybrid job created through `CreateJob`.
///
/// Every member `GetJob` reports is either stored here as the create supplied it or
/// computed from the timestamps below; nothing is kept twice.
#[derive(Debug, Clone)]
pub struct HybridJob {
    pub job_arn: String,
    pub job_name: String,
    /// A `JobPrimaryStatus` value.
    pub status: String,
    pub role_arn: String,
    pub algorithm_specification: AlgorithmSpecification,
    pub input_data_config: Vec<InputFileConfig>,
    pub output_data_config: JobOutputDataConfig,
    pub checkpoint_config: Option<JobCheckpointConfig>,
    pub stopping_condition: Option<JobStoppingCondition>,
    pub instance_config: InstanceConfig,
    pub hyper_parameters: BTreeMap<String, String>,
    pub device_config: DeviceConfig,
    pub associations: Vec<Association>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// When the job entered `RUNNING`; `None` while it is still queued.
    pub started_at: Option<SystemTime>,
    /// When the job reached a terminal status; `None` until it does.
    pub ended_at: Option<SystemTime>,
    pub failure_reason: Option<String>,
    /// When the `QUEUED` -> `RUNNING` transition falls due.
    pub running_due: SystemTime,
    /// When the terminal transition falls due.
    pub terminal_due: SystemTime,
    /// When `CANCELLING` -> `CANCELLED` falls due; set by `CancelJob`.
    pub cancelled_due: Option<SystemTime>,
}

impl HybridJob {
    /// Whether the job is in a status it can never leave.
    pub fn is_terminal(&self) -> bool {
        self.status == STATUS_COMPLETED
            || self.status == STATUS_FAILED
            || self.status == STATUS_CANCELLED
    }

    /// Whether the job still occupies the jobs queue.
    pub fn is_queued(&self) -> bool {
        self.status == STATUS_QUEUED
    }
}

/// An `@idempotencyToken` this service has seen, and what it minted.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The digest of the request the token was first used with, so a token reused for
    /// a different request is refused rather than answered with the first ARN. The
    /// operation's own name takes part, so one token used for two different creates is
    /// a reuse rather than a replay.
    pub fingerprint: String,
    /// The ARN that first use minted — a quantum task's, a hybrid job's or a spending
    /// limit's.
    pub resource_arn: String,
    /// When the token was first seen — what [`IDEMPOTENCY_TTL`] is measured from.
    pub recorded_at: SystemTime,
}

/// A spending limit created through `CreateSpendingLimit`.
///
/// Every member `SearchSpendingLimits` reports is either held here or computed from
/// the quantum tasks the device has (see
/// [`ComAmazonawsBraketBraketState::spend_on_device`]).
#[derive(Debug, Clone)]
pub struct SpendingLimit {
    pub spending_limit_arn: String,
    pub device_arn: String,
    /// The maximum that may be spent, in USD — the caller's own `spendingLimit`
    /// string, kept verbatim so the value read back is the value written.
    pub spending_limit: String,
    /// The period the limit is active over: exactly the `TimePeriod` the caller sent,
    /// or the [`DEFAULT_SPENDING_LIMIT_WINDOW`] when it sent none.
    pub time_period: TimePeriod,
    pub created_at: SystemTime,
    /// When the limit was last changed; equal to `created_at` until an update moves it.
    pub updated_at: SystemTime,
    pub tags: BTreeMap<String, String>,
}

/// Per-service state for com-amazonaws-braket-braket. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsBraketBraketState {
    /// The seeded device inventory, keyed by device ARN and ordered by it.
    pub devices: BTreeMap<String, Device>,
    /// Every quantum task created in this emulator, keyed by its ARN.
    pub quantum_tasks: BTreeMap<String, QuantumTask>,
    /// Every spending limit created in this emulator, keyed by its ARN and bounded by
    /// [`MAX_SPENDING_LIMITS`].
    pub spending_limits: BTreeMap<String, SpendingLimit>,
    /// `clientToken` -> what that token minted, so replaying an `@idempotencyToken`
    /// returns the first ARN instead of creating a second resource. Bounded: see
    /// [`ComAmazonawsBraketBraketState::prune_idempotency`].
    pub idempotency: BTreeMap<String, IdempotencyRecord>,
    /// Every hybrid job created in this emulator, keyed by its ARN.
    pub jobs: BTreeMap<String, HybridJob>,
}

impl Default for ComAmazonawsBraketBraketState {
    fn default() -> Self {
        Self {
            devices: seeded_devices(),
            quantum_tasks: BTreeMap::new(),
            spending_limits: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            jobs: BTreeMap::new(),
        }
    }
}

impl ComAmazonawsBraketBraketState {
    /// Materialise every status transition the clock has already made due.
    ///
    /// This is the one write a reporting operation may make: each transition was
    /// owed at `now` whether or not anybody called, so applying it changes no
    /// answer a caller could otherwise have seen.
    pub fn apply_due_transitions(&mut self, now: SystemTime) {
        for task in self.quantum_tasks.values_mut() {
            apply_due_transitions_to(task, now);
        }
        for job in self.jobs.values_mut() {
            apply_due_transitions_to_job(job, now);
        }
    }

    /// The jobs still waiting in the jobs queue, oldest first — the queue a hybrid
    /// job's position is counted in.
    ///
    /// Call [`apply_due_transitions`](Self::apply_due_transitions) first, so a job the
    /// clock has already started is not still reported as waiting.
    pub fn queued_jobs(&self) -> Vec<&HybridJob> {
        let mut queued: Vec<&HybridJob> =
            self.jobs.values().filter(|job| job.is_queued()).collect();
        queued.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.job_arn.cmp(&b.job_arn))
        });
        queued
    }

    /// How many hybrid jobs have not reached a terminal status — the number
    /// [`MAX_JOBS_IN_FLIGHT`] bounds.
    ///
    /// Call [`apply_due_transitions`](Self::apply_due_transitions) first, so a job the
    /// clock has already finished does not hold a slot.
    pub fn jobs_in_flight(&self) -> usize {
        self.jobs.values().filter(|job| !job.is_terminal()).count()
    }

    /// Make room for one more job record, the way
    /// [`evict_finished_tasks_over_capacity`](Self::evict_finished_tasks_over_capacity)
    /// makes room for one more task: drop the oldest TERMINAL jobs, with the
    /// idempotency tokens that name them, until the store holds at most
    /// `MAX_JOBS - 1`.
    pub fn evict_finished_jobs_over_capacity(&mut self) {
        if self.jobs.len() < MAX_JOBS {
            return;
        }
        let mut finished: Vec<(SystemTime, String)> = self
            .jobs
            .values()
            .filter(|job| job.is_terminal())
            .map(|job| (job.ended_at.unwrap_or(job.created_at), job.job_arn.clone()))
            .collect();
        finished.sort();
        for (_, arn) in finished {
            if self.jobs.len() < MAX_JOBS {
                break;
            }
            self.jobs.remove(&arn);
            self.idempotency
                .retain(|_, record| record.resource_arn != arn);
        }
    }

    /// The tasks still queued on `device_arn`, oldest first — the queue a
    /// position is counted in.
    pub fn queued_tasks_on(&self, device_arn: &str) -> Vec<&QuantumTask> {
        let mut queued: Vec<&QuantumTask> = self
            .quantum_tasks
            .values()
            .filter(|t| t.device_arn == device_arn && t.is_queued())
            .collect();
        queued.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.quantum_task_arn.cmp(&b.quantum_task_arn))
        });
        queued
    }

    /// How many tasks on `device_arn` have not reached a terminal status — the
    /// number [`MAX_TASKS_IN_FLIGHT_PER_DEVICE`] bounds.
    ///
    /// Call [`apply_due_transitions`](Self::apply_due_transitions) first, so a task
    /// the clock has already finished is not counted as in flight.
    pub fn tasks_in_flight_on(&self, device_arn: &str) -> usize {
        self.quantum_tasks
            .values()
            .filter(|t| t.device_arn == device_arn && !t.is_terminal())
            .count()
    }

    /// Make room for one more task record: drop the oldest FINISHED ones, with the
    /// idempotency tokens that name them, until the store holds at most
    /// `MAX_QUANTUM_TASKS - 1` — so after the insert that follows, the store holds at
    /// most [`MAX_QUANTUM_TASKS`] records however many tasks are created, and the
    /// per-request sweep over it stays bounded with it.
    ///
    /// Only terminal records are dropped, and oldest first, so a task still in flight
    /// is never forgotten and the most recent history is the history that is kept.
    pub fn evict_finished_tasks_over_capacity(&mut self) {
        if self.quantum_tasks.len() < MAX_QUANTUM_TASKS {
            return;
        }
        let mut finished: Vec<(SystemTime, String)> = self
            .quantum_tasks
            .values()
            .filter(|task| task.is_terminal())
            .map(|task| (task.ended_at.unwrap_or(task.created_at), task.quantum_task_arn.clone()))
            .collect();
        finished.sort();
        for (_, arn) in finished {
            if self.quantum_tasks.len() < MAX_QUANTUM_TASKS {
                break;
            }
            self.quantum_tasks.remove(&arn);
            self.idempotency
                .retain(|_, record| record.resource_arn != arn);
        }
    }

    /// Make room for one more `clientToken`: drop the entries that can no longer serve
    /// a replay, and then the oldest of what is left, until the index holds at most
    /// `MAX_IDEMPOTENCY_TOKENS - 1` — so after the insert that follows it holds at most
    /// [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// An entry is of no further use once the resource it names has been evicted or
    /// deleted, or once it is older than [`IDEMPOTENCY_TTL`] — so an endless stream of
    /// fresh tokens cannot grow this index without limit even though every create
    /// stores one.
    pub fn prune_idempotency(&mut self, now: SystemTime) {
        let tasks = &self.quantum_tasks;
        let jobs = &self.jobs;
        let spending_limits = &self.spending_limits;
        self.idempotency.retain(|_, record| {
            if !tasks.contains_key(&record.resource_arn)
                && !jobs.contains_key(&record.resource_arn)
                && !spending_limits.contains_key(&record.resource_arn)
            {
                return false;
            }
            match now.duration_since(record.recorded_at) {
                Ok(age) => age < IDEMPOTENCY_TTL,
                // Recorded in the future (the clock moved back): keep it, its TTL has
                // not run out.
                Err(_) => true,
            }
        });
        if self.idempotency.len() < MAX_IDEMPOTENCY_TOKENS {
            return;
        }
        let mut by_age: Vec<(SystemTime, String)> = self
            .idempotency
            .iter()
            .map(|(token, record)| (record.recorded_at, token.clone()))
            .collect();
        by_age.sort();
        for (_, token) in by_age {
            if self.idempotency.len() < MAX_IDEMPOTENCY_TOKENS {
                break;
            }
            self.idempotency.remove(&token);
        }
    }

    /// What one device's quantum tasks add up to over `period`, in USD: the amount
    /// still queued for spending, and the amount already spent.
    ///
    /// A quantum task is charged at the price the device's own capability document
    /// publishes ([`Device::cost_price`]), for the shots the task asked for, and only
    /// tasks created inside `period` count towards that period's figures:
    ///
    /// * a task that has not reached a terminal status is money COMMITTED but not yet
    ///   charged, so it counts towards the queued amount;
    /// * a task that COMPLETED has been charged, so it counts towards the total.
    ///
    /// A `CANCELLED` or `FAILED` task is charged for nothing, so only the tasks whose
    /// run produced a result count towards the total ([`TaskOutcome`]). The figures are
    /// computed from the records, so a device with queued work reports what that work has
    /// committed and a device whose tasks completed reports what they cost.
    ///
    /// A device charged per [`PRICE_UNIT_MINUTE`] contributes nothing: the emulator
    /// runs no simulation, so there is no duration to charge. Only a per-shot device
    /// can carry a spending limit at all (`CreateSpendingLimit` refuses a simulator).
    ///
    /// Call [`apply_due_transitions`](Self::apply_due_transitions) first, so the
    /// statuses these sums read are the ones every other call reports at this instant.
    pub fn spend_on_device(&self, device_arn: &str, period: &TimePeriod) -> (f64, f64) {
        let per_shot = self
            .devices
            .get(device_arn)
            .filter(|device| device.cost_unit == PRICE_UNIT_SHOT)
            .map_or(0.0, |device| device.cost_price);
        let from = date_time_as_nanos(&period.start_at);
        let to = date_time_as_nanos(&period.end_at);
        let mut queued = 0.0;
        let mut total = 0.0;
        for task in self.quantum_tasks.values() {
            if task.device_arn != device_arn {
                continue;
            }
            let created = date_time_as_nanos(&to_date_time(task.created_at));
            if created < from || created > to {
                continue;
            }
            #[allow(clippy::cast_precision_loss)]
            let charge = per_shot * task.shots as f64;
            if task.is_queued() {
                queued += charge;
            } else if task.status == STATUS_COMPLETED {
                total += charge;
            }
        }
        (queued, total)
    }

    /// The tag set stored on the resource `resource_arn` names, or `None` when no
    /// resource this service holds carries that ARN.
    ///
    /// Every store whose records carry tags is reached from here — the quantum tasks
    /// `CreateQuantumTask` writes, the hybrid jobs `CreateJob` writes and the spending
    /// limits `CreateSpendingLimit` writes — and the ARN itself says which one holds the
    /// resource, because each of those stores is keyed by it. ONE lookup for the tagging
    /// doors and the reads that report tags, so what `TagResource` and `UntagResource`
    /// write is what `ListTagsForResource` and the resource's own read report.
    ///
    /// A device resolves to `None`: devices are published by AWS rather than created
    /// through this API and hold no tag set at all.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        self.quantum_tasks
            .get(resource_arn)
            .map(|task| &task.tags)
            .or_else(|| self.jobs.get(resource_arn).map(|job| &job.tags))
            .or_else(|| {
                self.spending_limits
                    .get(resource_arn)
                    .map(|limit| &limit.tags)
            })
    }

    /// The tag set [`tags_of`](Self::tags_of) resolves, to WRITE: the same lookup over
    /// the same stores, so a door that changes a resource's tags and a door that reports
    /// them cannot disagree about which resource an ARN names.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(task) = self.quantum_tasks.get_mut(resource_arn) {
            return Some(&mut task.tags);
        }
        if let Some(job) = self.jobs.get_mut(resource_arn) {
            return Some(&mut job.tags);
        }
        Some(&mut self.spending_limits.get_mut(resource_arn)?.tags)
    }
}

/// How many fractional digits a COMPUTED amount is rendered to before
/// [`plain_amount`] drops the zeros it does not need.
///
/// A charge is a published per-unit price times an integer count, and the finest price
/// this catalog publishes carries five fractional digits ([`DEVICE_SEEDS`]), so a sum of
/// charges is a multiple of one hundred-thousandth of a dollar: six digits render such a
/// sum exactly while keeping the binary-float residue of the addition out of the text.
const COMPUTED_AMOUNT_DIGITS: usize = 6;

/// A COMPUTED amount in USD, as every Braket spending member reports one: the plain
/// decimal value, in the form [`plain_amount`] defines.
pub fn usd(amount: f64) -> String {
    if !amount.is_finite() || amount == 0.0 {
        // A sum of no charges, and the negative zero an empty sum can hold, are both
        // nothing spent.
        return "0".to_string();
    }
    let digits = COMPUTED_AMOUNT_DIGITS;
    plain_amount(&format!("{amount:.digits$}"))
}

/// One USD amount in the plain decimal form Amazon Braket reports a spending member in:
/// the amount's own digits, with the insignificant zeros of its fraction — and a
/// fraction left empty by them — dropped.
///
/// Nothing is padded to a fixed width, so cents appear exactly when they are not zero:
/// a limit submitted as a whole number of dollars is reported as that whole number, and a
/// device with nothing spent on it reports `0`. The model puts no `@pattern` and no
/// length on `SpendingLimitSummary$spendingLimit`, `$queuedSpend` or `$totalSpend`
/// (`smithy_query(query="shape", shape="SpendingLimitSummary")`, which reports them as
/// plain required Strings), and the recorded search is the statement of the form: an
/// amount whose cents are zero comes back without them, and an unspent device comes back
/// as a bare zero.
///
/// Takes the TEXT, so the amount a caller submitted and an amount computed from the
/// records are reported in one form without routing a submitted amount through a float.
pub fn plain_amount(amount: &str) -> String {
    let Some((whole, fraction)) = amount.split_once('.') else {
        return amount.to_string();
    };
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        return whole.to_string();
    }
    format!("{whole}.{fraction}")
}

/// Apply the transitions `now` has made due to one hybrid job.
///
/// A job leaves `QUEUED` for `RUNNING` when [`JOB_RUNNING_SPAN`] is up, and ends at
/// [`JOB_TERMINAL_SPAN`] in `FAILED`: nothing ran the algorithm, so
/// [`JOB_NOT_EXECUTED_FAILURE_REASON`] is the reason it carries.
fn apply_due_transitions_to_job(job: &mut HybridJob, now: SystemTime) {
    if job.status == STATUS_CANCELLING {
        // A cancellation the clock has finished leaves the job cancelled, exactly as one
        // of a quantum task does.
        if let Some(due) = job.cancelled_due {
            if now >= due {
                job.status = STATUS_CANCELLED.to_string();
                job.ended_at = Some(due);
            }
        }
        return;
    }
    if job.is_terminal() {
        return;
    }
    if now >= job.running_due && job.started_at.is_none() {
        job.started_at = Some(job.running_due);
    }
    if now >= job.terminal_due {
        job.status = STATUS_FAILED.to_string();
        job.ended_at = Some(job.terminal_due);
        job.failure_reason = Some(JOB_NOT_EXECUTED_FAILURE_REASON.to_string());
    } else if now >= job.running_due {
        job.status = STATUS_RUNNING.to_string();
    }
}

/// Apply the transitions `now` has made due to one task.
fn apply_due_transitions_to(task: &mut QuantumTask, now: SystemTime) {
    if task.status == STATUS_CANCELLING {
        if let Some(due) = task.cancelled_due {
            if now >= due {
                task.status = STATUS_CANCELLED.to_string();
                task.ended_at = Some(due);
            }
        }
        return;
    }
    if !task.is_queued() {
        return;
    }
    if now >= task.terminal_due {
        // Which terminal status is owed was decided by the run itself: a task whose
        // program ran and whose result was written completes, and one that produced no
        // result ends in the failure status carrying the reason.
        task.status = task.outcome.terminal_status().to_string();
        task.ended_at = Some(task.terminal_due);
        task.failure_reason = match &task.outcome {
            TaskOutcome::Completed { .. } => None,
            TaskOutcome::Failed { reason } => Some(reason.clone()),
        };
    } else if now >= task.queued_due {
        task.status = STATUS_QUEUED.to_string();
    }
}

// ---------------------------------------------------------------------------
// The seeded device inventory
// ---------------------------------------------------------------------------

/// The `actionType` of the OpenQASM program action.
pub const ACTION_OPENQASM: &str = "braket.ir.openqasm.program";
/// The `actionType` of the (legacy) JAQCD program action.
pub const ACTION_JAQCD: &str = "braket.ir.jaqcd.program";
/// The `actionType` of the analog Hamiltonian-simulation program action.
pub const ACTION_AHS: &str = "braket.ir.ahs.program";
/// The `actionType` of the annealing-problem action: a quantum annealer is given a
/// problem to minimise rather than a program to run.
pub const ACTION_ANNEALING: &str = "braket.ir.annealing.problem";
/// The `actionType` of the photonic (Blackbird) program action.
pub const ACTION_BLACKBIRD: &str = "braket.ir.blackbird.program";

/// Whether an action is a GATE-MODEL program action.
///
/// Only a program of GATES can name operations, result types and pragmas, so only a
/// gate-model action publishes them; every other action family (an analog Hamiltonian,
/// an annealing problem, a photonic program) publishes its action type and the schema
/// versions it accepts and nothing more.
fn is_gate_model_action(action_type: &str) -> bool {
    action_type == ACTION_OPENQASM || action_type == ACTION_JAQCD
}

/// The schema versions a device action in this inventory accepts.
///
/// `version` in a Braket device-action entry is the LIST of `major.minor` program
/// schema versions the action accepts, as the BraketSchemas package numbers them —
/// so a client that keys off it reads `"1.0"`, not a bare major number.
const ACTION_SCHEMA_VERSIONS: &[&str] = &["1.0"];

/// `maximumQubitArrays` / `maximumClassicalArrays`: how many qubit and bit arrays one
/// OpenQASM program may declare.
///
/// Both are properties of the OpenQASM device-action schema rather than of a device,
/// so every device that accepts [`ACTION_OPENQASM`] publishes the same values.
const MAXIMUM_QUBIT_ARRAYS: u32 = 1;
/// See [`MAXIMUM_QUBIT_ARRAYS`].
const MAXIMUM_CLASSICAL_ARRAYS: u32 = 1;
/// `forbiddenArrayOperations`: the array expressions no OpenQASM program may use.
///
/// A property of the OpenQASM device-action schema, as [`MAXIMUM_QUBIT_ARRAYS`] is.
const FORBIDDEN_ARRAY_OPERATIONS: &[&str] = &[
    "concatenation",
    "negativeIndex",
    "range",
    "rangeWithStep",
    "slice",
];

/// The observables a measured result type may be taken against — the Pauli bases,
/// the Hadamard basis, the identity, and a caller-supplied Hermitian matrix.
const OBSERVABLES: &[&str] = &["x", "y", "z", "h", "i", "hermitian"];

/// The `braket_noise_*` pragmas, one per noise channel an OpenQASM program can apply.
///
/// Applying a noise channel needs a device that MODELS noise (a density-matrix
/// simulator), so these are supported exactly where [`DeviceSeed::noise`] is set and
/// forbidden everywhere else.
const NOISE_PRAGMAS: &[&str] = &[
    "braket_noise_amplitude_damping",
    "braket_noise_bit_flip",
    "braket_noise_depolarizing",
    "braket_noise_generalized_amplitude_damping",
    "braket_noise_kraus",
    "braket_noise_pauli_channel",
    "braket_noise_phase_damping",
    "braket_noise_phase_flip",
    "braket_noise_two_qubit_dephasing",
    "braket_noise_two_qubit_depolarizing",
];

/// Every result type a Braket program can ask for, with the `braket_result_type_*`
/// pragma that requests it in an OpenQASM program (the result-type name in
/// snake_case). A device publishes the pragma as supported exactly when it publishes
/// the result type, so one catalog decides both.
const RESULT_TYPE_PRAGMAS: &[(&str, &str)] = &[
    ("Sample", "braket_result_type_sample"),
    ("Expectation", "braket_result_type_expectation"),
    ("Variance", "braket_result_type_variance"),
    ("Probability", "braket_result_type_probability"),
    ("StateVector", "braket_result_type_state_vector"),
    ("DensityMatrix", "braket_result_type_density_matrix"),
    ("Amplitude", "braket_result_type_amplitude"),
    ("AdjointGradient", "braket_result_type_adjoint_gradient"),
];

/// `verbatim`: run a box of native gates exactly as written, with no recompilation.
const VERBATIM_PRAGMA: &str = "verbatim";
/// `braket_unitary_matrix`: apply a caller-supplied unitary, legal exactly where the
/// device accepts the `unitary` operation.
const UNITARY_MATRIX_PRAGMA: &str = "braket_unitary_matrix";

/// One result type a device publishes in its action properties.
struct ResultTypeSeed {
    /// The result-type name the Braket schemas publish; each one is the left half of
    /// a [`RESULT_TYPE_PRAGMAS`] row.
    name: &'static str,
    /// The observables it may be measured against; empty for the result types that
    /// take none.
    observables: &'static [&'static str],
    /// Whether the device computes the value exactly from the full state rather than
    /// sampling it, which is only possible on a shot-less run — so an exact result
    /// type publishes `minShots`/`maxShots` of 0 and a sampled one the device's own
    /// shot range.
    exact: bool,
}

/// The result types a state-vector simulator computes: everything sampled, plus the
/// exact state and gradient a full state vector makes available.
const STATE_VECTOR_RESULT_TYPES: &[ResultTypeSeed] = &[
    ResultTypeSeed { name: "Sample", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Expectation", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Variance", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Probability", observables: &[], exact: false },
    ResultTypeSeed { name: "Amplitude", observables: &[], exact: true },
    ResultTypeSeed { name: "StateVector", observables: &[], exact: true },
    ResultTypeSeed { name: "AdjointGradient", observables: OBSERVABLES, exact: true },
];

/// The result types a density-matrix (noise) simulator computes: the density matrix
/// stands in for the state vector, so no amplitude or state vector is published.
const DENSITY_MATRIX_RESULT_TYPES: &[ResultTypeSeed] = &[
    ResultTypeSeed { name: "Sample", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Expectation", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Variance", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Probability", observables: &[], exact: false },
    ResultTypeSeed { name: "DensityMatrix", observables: &[], exact: true },
];

/// The result types a tensor-network simulator computes: it never holds the whole
/// state, so a single amplitude is the most it can report exactly.
const TENSOR_NETWORK_RESULT_TYPES: &[ResultTypeSeed] = &[
    ResultTypeSeed { name: "Sample", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Expectation", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Variance", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Probability", observables: &[], exact: false },
    ResultTypeSeed { name: "Amplitude", observables: &[], exact: true },
];

/// The result types a gate-model QPU computes: hardware only ever SAMPLES, so every
/// row needs shots and none is exact.
const QPU_RESULT_TYPES: &[ResultTypeSeed] = &[
    ResultTypeSeed { name: "Sample", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Expectation", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Variance", observables: OBSERVABLES, exact: false },
    ResultTypeSeed { name: "Probability", observables: &[], exact: false },
];

/// The gate-model operations a program submitted to this inventory may use, in the
/// names the Braket device schemas publish them under (Amazon Braket's supported-gates
/// documentation). Braket compiles a program to the device's own
/// [`DeviceSeed::native_gates`], so a gate-model device accepts the whole set.
const GATE_MODEL_OPERATIONS: &[&str] = &[
    "barrier",
    "ccnot",
    "cnot",
    "cphaseshift",
    "cphaseshift00",
    "cphaseshift01",
    "cphaseshift10",
    "cswap",
    "cv",
    "cy",
    "cz",
    "ecr",
    "gphase",
    "gpi",
    "gpi2",
    "h",
    "i",
    "iswap",
    "measure",
    "ms",
    "phaseshift",
    "prx",
    "pswap",
    "reset",
    "rx",
    "ry",
    "rz",
    "s",
    "si",
    "swap",
    "t",
    "ti",
    "u",
    "unitary",
    "v",
    "vi",
    "x",
    "xx",
    "xy",
    "y",
    "yy",
    "z",
    "zz",
];

/// Where `deviceDocumentation.externalDocumentationUrl` points: the Amazon Braket
/// developer guide, whose supported-devices page documents every published device.
const DOCUMENTATION_BASE: &str = "https://docs.aws.amazon.com/braket/latest/developerguide";
/// The page of that guide the devices are documented on, which each device's own link
/// anchors on its [`DeviceSeed::documentation_section`].
const DEVICE_LIST_PAGE: &str = "braket-devices.html";

/// One row of the seeded inventory.
///
/// Every property the device's capability document publishes comes from here, so the
/// catalog a caller reads through `GetDevice` and the rules `CreateQuantumTask` holds
/// it to are the same data.
struct DeviceSeed {
    arn: &'static str,
    name: &'static str,
    provider: &'static str,
    device_type: &'static str,
    device_status: &'static str,
    qubit_count: u32,
    shots_range: (i64, i64),
    action_types: &'static [&'static str],
    /// The operations a program for this device may use (`supportedOperations`);
    /// empty for a device whose programs name no gates at all.
    supported_operations: &'static [&'static str],
    /// The operations the device executes without recompilation
    /// (`paradigm.nativeGateSet`) — a subset of `supported_operations`. Empty for a
    /// device with no native gate layer beneath the one a program is written in.
    native_gates: &'static [&'static str],
    /// The result types the device computes (`supportedResultTypes`), which also
    /// decide its `braket_result_type_*` pragmas.
    result_types: &'static [ResultTypeSeed],
    /// Whether the device runs a `verbatim` box: real hardware can be addressed in
    /// its native gates, a simulator has no native gate layer to bypass.
    verbatim: bool,
    /// Whether the device applies a noise channel a program asks for, which decides
    /// its [`NOISE_PRAGMAS`].
    noise: bool,
    /// `deviceCost`: what a run costs, as a price and the unit it is charged per.
    /// A catalog figure from Amazon Braket's pricing page: simulators are charged
    /// per minute of simulation, QPUs per shot.
    cost: (f64, &'static str),
    /// `deviceDocumentation.summary`.
    summary: &'static str,
    /// `deviceDocumentation.externalDocumentationUrl`: the section of the guide's
    /// supported-devices page ([`DEVICE_LIST_PAGE`]) that documents THIS device.
    ///
    /// A published link of the catalog entry, carried per device rather than derived
    /// from the device's name: the guide names a device's section after the family or
    /// the provider it belongs to as often as after the device itself, so a link built
    /// out of the name leads nowhere.
    documentation_section: &'static str,
    /// `deviceDocumentation.imageUrl`: the device's published image asset, the other
    /// link the catalog entry carries and the Braket SDK surfaces.
    image_url: &'static str,
    /// `service.updatedAt`: when this catalog entry was last published, in the
    /// `yyyy-mm-ddThh:mm:ss` form the device-service-properties document carries it in.
    ///
    /// A FIXED property of the published entry, not an instant this emulator observes:
    /// devices are published by Amazon Braket rather than created through the API, so
    /// the same device read after a restart must report the same document — anything
    /// that caches or diffs the catalog would otherwise see a change nobody made.
    published_at: &'static str,
}

/// The device inventory this emulator publishes.
///
/// The ARNs, names and providers follow Amazon Braket's published supported-devices
/// list (simulator ARNs carry no region or account; a QPU's ARN carries the region
/// that hosts it). A real device's status tracks hardware availability windows,
/// which the emulator does not model, so each row's status is a FIXED seed chosen so
/// every modelled `DeviceStatus` is reachable — `ONLINE` devices accept tasks, and
/// the rows that are not online are what `CreateQuantumTask` refuses with
/// `DeviceOfflineException` / `DeviceRetiredException`. The status of a row that is not
/// taking work also decides the reason the catalog publishes beside it (see
/// [`status_reason`]).
///
/// A device Amazon Braket has RETIRED stays in the list it publishes, carrying that
/// status, which is what lets a caller naming one be told the device is retired rather
/// than that no such device exists; so this inventory holds retired devices of every
/// program family the list publishes — gate-model, analog, annealing and photonic — and
/// not only the ones still taking work.
const DEVICE_SEEDS: &[DeviceSeed] = &[
    DeviceSeed {
        arn: "arn:aws:braket:::device/quantum-simulator/amazon/sv1",
        name: "SV1",
        provider: "Amazon Braket",
        device_type: DEVICE_TYPE_SIMULATOR,
        device_status: DEVICE_ONLINE,
        qubit_count: 34,
        shots_range: (0, 100_000),
        action_types: &[ACTION_OPENQASM, ACTION_JAQCD],
        supported_operations: GATE_MODEL_OPERATIONS,
        // A simulator applies every operation it accepts directly: it has no native
        // gate layer under the one a program is written in, which is also why it
        // forbids `verbatim`.
        native_gates: &[],
        result_types: STATE_VECTOR_RESULT_TYPES,
        verbatim: false,
        noise: false,
        cost: (0.075, "minute"),
        summary: "Amazon Braket state vector simulator",
        documentation_section: "braket-simulator-sv1",
        image_url: "https://d1.awsstatic.com/braket/devices/amazon-sv1.png",
        published_at: "2020-08-13T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:::device/quantum-simulator/amazon/dm1",
        name: "DM1",
        provider: "Amazon Braket",
        device_type: DEVICE_TYPE_SIMULATOR,
        device_status: DEVICE_ONLINE,
        qubit_count: 17,
        shots_range: (0, 100_000),
        action_types: &[ACTION_OPENQASM, ACTION_JAQCD],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &[],
        result_types: DENSITY_MATRIX_RESULT_TYPES,
        verbatim: false,
        noise: true,
        cost: (0.075, "minute"),
        summary: "Amazon Braket density matrix simulator",
        documentation_section: "braket-simulator-dm1",
        image_url: "https://d1.awsstatic.com/braket/devices/amazon-dm1.png",
        published_at: "2021-05-26T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:::device/quantum-simulator/amazon/tn1",
        name: "TN1",
        provider: "Amazon Braket",
        device_type: DEVICE_TYPE_SIMULATOR,
        device_status: DEVICE_ONLINE,
        qubit_count: 50,
        shots_range: (1, 1_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &[],
        result_types: TENSOR_NETWORK_RESULT_TYPES,
        verbatim: false,
        noise: false,
        cost: (0.275, "minute"),
        summary: "Amazon Braket tensor network simulator",
        documentation_section: "braket-simulator-tn1",
        image_url: "https://d1.awsstatic.com/braket/devices/amazon-tn1.png",
        published_at: "2021-02-10T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/ionq/Aria-1",
        name: "Aria 1",
        provider: "IonQ",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 25,
        shots_range: (1, 1_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["gpi", "gpi2", "ms"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.03, "shot"),
        summary: "IonQ Aria trapped-ion quantum processor",
        documentation_section: "braket-qpu-partner-ionq",
        image_url: "https://d1.awsstatic.com/braket/devices/ionq-aria-1.png",
        published_at: "2023-06-28T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/ionq/Forte-1",
        name: "Forte 1",
        provider: "IonQ",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_OFFLINE,
        qubit_count: 36,
        shots_range: (1, 1_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["gpi", "gpi2", "ms"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.08, "shot"),
        summary: "IonQ Forte trapped-ion quantum processor",
        documentation_section: "braket-qpu-partner-ionq",
        image_url: "https://d1.awsstatic.com/braket/devices/ionq-forte-1.png",
        published_at: "2024-02-28T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:eu-north-1::device/qpu/iqm/Garnet",
        name: "Garnet",
        provider: "IQM",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 20,
        shots_range: (1, 10_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["cz", "prx"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.00145, "shot"),
        summary: "IQM Garnet superconducting quantum processor",
        documentation_section: "braket-qpu-partner-iqm",
        image_url: "https://d1.awsstatic.com/braket/devices/iqm-garnet.png",
        published_at: "2024-03-26T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:eu-north-1::device/qpu/iqm/Emerald",
        name: "Emerald",
        provider: "IQM",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_PAUSED,
        qubit_count: 54,
        shots_range: (1, 10_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["cz", "prx"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.00145, "shot"),
        summary: "IQM Emerald superconducting quantum processor",
        documentation_section: "braket-qpu-partner-iqm",
        image_url: "https://d1.awsstatic.com/braket/devices/iqm-emerald.png",
        published_at: "2025-02-25T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-west-1::device/qpu/rigetti/Ankaa-3",
        name: "Ankaa-3",
        provider: "Rigetti",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 84,
        shots_range: (1, 10_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["cz", "iswap", "rx", "rz"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.0009, "shot"),
        summary: "Rigetti Ankaa superconducting quantum processor",
        documentation_section: "braket-qpu-partner-rigetti",
        image_url: "https://d1.awsstatic.com/braket/devices/rigetti-ankaa-3.png",
        published_at: "2024-12-17T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/quera/Aquila",
        name: "Aquila",
        provider: "QuEra",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_ONLINE,
        qubit_count: 256,
        shots_range: (1, 1_000),
        // An analog Hamiltonian-simulation device: a program for it is a register
        // layout and a driving field, so it names no gates, no pragmas and no result
        // types at all.
        action_types: &[ACTION_AHS],
        supported_operations: &[],
        native_gates: &[],
        result_types: &[],
        verbatim: false,
        noise: false,
        cost: (0.01, "shot"),
        summary: "QuEra Aquila neutral-atom analog Hamiltonian simulator",
        documentation_section: "braket-qpu-partner-quera",
        image_url: "https://d1.awsstatic.com/braket/devices/quera-aquila.png",
        published_at: "2022-11-28T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:eu-west-2::device/qpu/oqc/Lucy",
        name: "Lucy",
        provider: "Oxford Quantum Circuits",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 8,
        shots_range: (1, 10_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["ecr", "i", "rz", "v", "x"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.00035, "shot"),
        summary: "Oxford Quantum Circuits Lucy superconducting quantum processor",
        documentation_section: "braket-qpu-partner-oqc",
        image_url: "https://d1.awsstatic.com/braket/devices/oqc-lucy.png",
        published_at: "2022-03-29T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-west-1::device/qpu/rigetti/Aspen-M-3",
        name: "Aspen-M-3",
        provider: "Rigetti",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 79,
        shots_range: (1, 10_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["cphaseshift", "cz", "rx", "rz", "xy"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.00035, "shot"),
        summary: "Rigetti Aspen-M-3 superconducting quantum processor",
        documentation_section: "braket-qpu-partner-rigetti",
        image_url: "https://d1.awsstatic.com/braket/devices/rigetti-aspen-m-3.png",
        published_at: "2022-10-17T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:::device/qpu/ionq/Harmony",
        name: "Harmony",
        provider: "IonQ",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 11,
        shots_range: (1, 10_000),
        action_types: &[ACTION_OPENQASM],
        supported_operations: GATE_MODEL_OPERATIONS,
        native_gates: &["gpi", "gpi2", "ms"],
        result_types: QPU_RESULT_TYPES,
        verbatim: true,
        noise: false,
        cost: (0.01, "shot"),
        summary: "IonQ Harmony trapped-ion quantum processor",
        documentation_section: "braket-qpu-partner-ionq",
        image_url: "https://d1.awsstatic.com/braket/devices/ionq-harmony.png",
        published_at: "2020-08-13T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-west-2::device/qpu/d-wave/Advantage_system6",
        name: "Advantage_system6",
        provider: "D-Wave",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 5_760,
        shots_range: (1, 10_000),
        // An annealer is given a problem to MINIMISE rather than a program to run, so
        // its action names no gates, no pragmas and no result types at all.
        action_types: &[ACTION_ANNEALING],
        supported_operations: &[],
        native_gates: &[],
        result_types: &[],
        verbatim: false,
        noise: false,
        cost: (0.00019, "shot"),
        summary: "D-Wave Advantage quantum annealer",
        documentation_section: "braket-qpu-partner-dwave",
        image_url: "https://d1.awsstatic.com/braket/devices/dwave-advantage-system6.png",
        published_at: "2022-02-23T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:::device/qpu/d-wave/DW_2000Q_6",
        name: "DW_2000Q_6",
        provider: "D-Wave",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        qubit_count: 2_048,
        shots_range: (1, 10_000),
        action_types: &[ACTION_ANNEALING],
        supported_operations: &[],
        native_gates: &[],
        result_types: &[],
        verbatim: false,
        noise: false,
        cost: (0.00019, "shot"),
        summary: "D-Wave 2000Q quantum annealer",
        documentation_section: "braket-qpu-partner-dwave",
        image_url: "https://d1.awsstatic.com/braket/devices/dwave-dw-2000q-6.png",
        published_at: "2020-08-13T00:00:00",
    },
    DeviceSeed {
        arn: "arn:aws:braket:us-east-1::device/qpu/xanadu/Borealis",
        name: "Borealis",
        provider: "Xanadu",
        device_type: DEVICE_TYPE_QPU,
        device_status: DEVICE_RETIRED,
        // A photonic device's register is its optical modes.
        qubit_count: 216,
        shots_range: (1, 100_000),
        action_types: &[ACTION_BLACKBIRD],
        supported_operations: &[],
        native_gates: &[],
        result_types: &[],
        verbatim: false,
        noise: false,
        cost: (0.00035, "shot"),
        summary: "Xanadu Borealis photonic quantum processor",
        documentation_section: "braket-qpu-partner-xanadu",
        image_url: "https://d1.awsstatic.com/braket/devices/xanadu-borealis.png",
        published_at: "2022-06-02T00:00:00",
    },
];

/// Build the seeded inventory, keyed and ordered by device ARN.
fn seeded_devices() -> BTreeMap<String, Device> {
    DEVICE_SEEDS
        .iter()
        .map(|seed| {
            let device = Device {
                device_arn: seed.arn.to_string(),
                device_name: seed.name.to_string(),
                provider_name: seed.provider.to_string(),
                device_type: seed.device_type.to_string(),
                device_status: seed.device_status.to_string(),
                device_status_reason: status_reason(seed),
                device_capabilities: capabilities_document(seed),
                shots_range: seed.shots_range,
                cost_price: seed.cost.0,
                cost_unit: seed.cost.1.to_string(),
            };
            (device.device_arn.clone(), device)
        })
        .collect()
}

/// Which reason the catalog publishes for a device in `device_status`, or `None` for a
/// status that has no reason to publish.
///
/// `DeviceStatusReasonCode` names the reasons a device is not taking work
/// (`smithy_query(query="shape",shape="DeviceStatusReasonCode")`): a device is
/// unavailable while the clock lies outside the execution windows its capability
/// document publishes, and held out of service while it is worked on. A device that is
/// `ONLINE` is taking work, and one the service has retired is gone for good rather than
/// for a reason that ends, so neither of those statuses carries a reason at all.
fn status_reason_code(device_status: &str) -> Option<&'static str> {
    match device_status {
        DEVICE_OFFLINE => Some(REASON_OUTSIDE_AVAILABILITY_WINDOW),
        DEVICE_PAUSED => Some(REASON_MAINTENANCE),
        _ => None,
    }
}

/// The status reason one catalog row publishes, for a row whose status has one.
///
/// Both instants are the row's OWN published instant ([`DeviceSeed::published_at`], the
/// `service.updatedAt` of its capability document): the reason has held since the entry
/// stating this status was published, and nothing has updated it since — so, like every
/// other value of the entry, the reason a caller reads is the same before and after a
/// restart rather than an instant this emulator observed. `estimatedEndAt` is left out,
/// which the model allows: this emulator runs no hardware whose schedule could estimate
/// when the device comes back, so it publishes no estimate.
fn status_reason(seed: &DeviceSeed) -> Option<DeviceStatusReason> {
    let reason = status_reason_code(seed.device_status)?;
    let published = published_instant(seed.published_at);
    Some(DeviceStatusReason {
        reason: reason.to_string(),
        started_at: Some(published),
        estimated_end_at: None,
        updated_at: published,
    })
}

/// A catalog row's published instant as the modelled `Timestamp` the wire carries.
///
/// The device-service-properties document carries that instant as a date and a time of
/// day with no zone offset; a published catalog instant is UTC, which is the offset the
/// `date-time` form the model declares needs to read it back.
fn published_instant(published_at: &str) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_str(
        &format!("{published_at}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .expect("every seeded published instant is written in the document's own form")
}

/// The `deviceCapabilities` JSON document for one seeded device.
///
/// A Braket device-capability document: the schema header; the `service` section with
/// the execution windows, the shot range, what a run costs (`deviceCost`) and where
/// the device is documented (`deviceDocumentation`); one `action` entry per action
/// type the device accepts, carrying the device-schema properties a client reads to
/// decide what it may submit (see [`action_properties`]); and the device's own
/// computing properties, under the key its family publishes them as (see
/// [`device_properties_section`]). Built with
/// `serde_json`, so what a caller parses is always well-formed JSON, and every
/// property is computed from the catalog row — the emulator has no hardware, so it
/// reports no calibration data.
///
/// Nothing in it comes from the clock or from the device's NAME: the documentation links
/// and the instant the entry was published are properties of the catalog row
/// ([`DeviceSeed::documentation_section`], [`DeviceSeed::image_url`],
/// [`DeviceSeed::published_at`]), so the document a caller reads for one device is the
/// same document before and after a restart.
fn capabilities_document(seed: &DeviceSeed) -> String {
    let actions: serde_json::Map<String, serde_json::Value> = seed
        .action_types
        .iter()
        .map(|action_type| {
            (
                (*action_type).to_string(),
                action_properties(seed, action_type),
            )
        })
        .collect();
    let mut document = serde_json::json!({
        "braketSchemaHeader": {
            "name": capabilities_schema(seed),
            "version": "1",
        },
        "service": {
            "braketSchemaHeader": {
                "name": "braket.device_schema.device_service_properties",
                "version": "1",
            },
            "executionWindows": [{
                "executionDay": "Everyday",
                "windowStartHour": "00:00:00",
                "windowEndHour": "23:59:59",
            }],
            "shotsRange": [seed.shots_range.0, seed.shots_range.1],
            "deviceCost": {
                "price": seed.cost.0,
                "unit": seed.cost.1,
            },
            "deviceDocumentation": {
                "imageUrl": seed.image_url,
                "summary": seed.summary,
                "externalDocumentationUrl":
                    format!(
                        "{DOCUMENTATION_BASE}/{DEVICE_LIST_PAGE}#{}",
                        seed.documentation_section
                    ),
            },
            "updatedAt": seed.published_at,
        },
        "action": serde_json::Value::Object(actions),
        "deviceParameters": device_parameters_schema(seed),
    });
    // The device's own computing properties go under the key its family publishes them
    // as, so a client reads the section the device-schema layout gives it.
    document
        .as_object_mut()
        .expect("the document above is a JSON object")
        .insert(
            device_properties_section(seed).to_string(),
            device_properties(seed),
        );
    document.to_string()
}

/// The key a device's own computing properties are published under.
///
/// A gate-model device computes with gates and an analog one with a Hamiltonian, and
/// both publish that as their `paradigm`; a device of any other family (an annealer, a
/// photonic processor) has no gate-model paradigm at all, and what it publishes in its
/// place is its provider's own properties.
fn device_properties_section(seed: &DeviceSeed) -> &'static str {
    if seed.action_types.contains(&ACTION_AHS)
        || seed.action_types.iter().any(|action| is_gate_model_action(action))
    {
        "paradigm"
    } else {
        "provider"
    }
}

/// The schema a device's capability document declares itself to be an instance of.
///
/// A client reads this header to pick the device-schema model it deserialises the rest
/// of the document with, so it names the concrete schema of the device's family: the
/// gate-model simulator schema for a simulator, and the provider's own device-
/// capability schema for a QPU, which lives in the module named after the provider
/// segment of the device's ARN.
fn capabilities_schema(seed: &DeviceSeed) -> String {
    if seed.device_type == DEVICE_TYPE_SIMULATOR {
        "braket.device_schema.simulators.gate_model_simulator_device_capabilities".to_string()
    } else {
        let namespace = provider_namespace(seed.arn);
        format!("braket.device_schema.{namespace}.{namespace}_device_capabilities")
    }
}

/// The schema the device-properties section declares itself to be an instance of: the
/// gate-model simulator or QPU paradigm, the provider's analog Hamiltonian-simulation
/// paradigm for a device whose programs are analog, and the provider's own device
/// properties for a family with no gate-model paradigm.
fn device_properties_schema(seed: &DeviceSeed) -> String {
    let namespace = provider_namespace(seed.arn);
    if seed.action_types.contains(&ACTION_AHS) {
        format!("braket.device_schema.{namespace}.{namespace}_ahs_paradigm_properties")
    } else if !seed.action_types.iter().any(|a| is_gate_model_action(a)) {
        // A device whose programs are neither gates nor an analog Hamiltonian — an
        // annealing problem, a photonic program — has no gate-model paradigm to
        // publish, so what it publishes is its provider's own device properties.
        format!("braket.device_schema.{namespace}.{namespace}_provider_properties")
    } else if seed.device_type == DEVICE_TYPE_SIMULATOR {
        "braket.device_schema.simulators.gate_model_simulator_paradigm_properties".to_string()
    } else {
        "braket.device_schema.gate_model_qpu_paradigm_properties".to_string()
    }
}

/// The provider segment of a device ARN — the `<provider>` of
/// `.../device/<kind>/<provider>/<name>` — as the module a provider's Braket device
/// schemas live in: the segment reduced to its letters and digits, because a schema
/// module name carries none of the separators an ARN segment may (the provider whose
/// ARN segment reads `d-wave` publishes `braket.device_schema.dwave`).
fn provider_namespace(arn: &str) -> String {
    arn.rsplit('/')
        .nth(1)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The JSON Schema of the `deviceParameters` a task for this device may carry.
///
/// `CreateQuantumTask` accepts any well-formed JSON document as a task's
/// `deviceParameters`, so the schema published here is the draft-07 document that says
/// exactly that, with the schema header a Braket device-parameter document carries. A
/// client that validates its parameters before submitting them gets a usable schema
/// rather than an empty object, and the emulator advertises no constraint it does not
/// enforce.
fn device_parameters_schema(seed: &DeviceSeed) -> serde_json::Value {
    serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "title": format!("{} device parameters", seed.name),
        "type": "object",
        "properties": {
            "braketSchemaHeader": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "version": { "type": "string" },
                },
            },
        },
        "additionalProperties": true,
    })
}

/// The `action` entry one device publishes for one action type.
///
/// What an entry carries is decided by the ACTION TYPE, because the properties are
/// the action schema's: a gate-model program action ([`ACTION_OPENQASM`],
/// [`ACTION_JAQCD`]) publishes the operations and result types a program may use, and
/// the OpenQASM one adds the pragmas it may carry and the array limits its schema
/// defines; every other action family carries neither, because a problem or an analog
/// program names no gates and no pragmas. Every entry carries its `actionType` and the
/// [`ACTION_SCHEMA_VERSIONS`] it accepts.
fn action_properties(seed: &DeviceSeed, action_type: &str) -> serde_json::Value {
    let mut entry = serde_json::Map::new();
    entry.insert("actionType".to_string(), serde_json::json!(action_type));
    entry.insert(
        "version".to_string(),
        serde_json::json!(ACTION_SCHEMA_VERSIONS),
    );
    if !is_gate_model_action(action_type) {
        return serde_json::Value::Object(entry);
    }
    entry.insert(
        "supportedOperations".to_string(),
        serde_json::json!(seed.supported_operations),
    );
    entry.insert(
        "supportedResultTypes".to_string(),
        supported_result_types(seed),
    );
    if action_type == ACTION_OPENQASM {
        let (supported, forbidden) = pragmas(seed);
        entry.insert("supportedPragmas".to_string(), serde_json::json!(supported));
        entry.insert("forbiddenPragmas".to_string(), serde_json::json!(forbidden));
        entry.insert(
            "maximumQubitArrays".to_string(),
            serde_json::json!(MAXIMUM_QUBIT_ARRAYS),
        );
        entry.insert(
            "maximumClassicalArrays".to_string(),
            serde_json::json!(MAXIMUM_CLASSICAL_ARRAYS),
        );
        entry.insert(
            "forbiddenArrayOperations".to_string(),
            serde_json::json!(FORBIDDEN_ARRAY_OPERATIONS),
        );
        // The hardware constraints a program must respect, which a client checks
        // before it builds one. A simulator imposes none of them: it applies the
        // program as written, to whichever qubits the program names. Hardware is
        // addressed as physical qubits on a contiguous register it measures as a
        // whole, and is the only thing a verbatim box or a rewiring switch means
        // anything to.
        let hardware = seed.device_type == DEVICE_TYPE_QPU;
        for (property, value) in [
            ("supportPhysicalQubits", hardware),
            ("requiresAllQubitsMeasurement", hardware),
            ("requiresContiguousQubitIndices", hardware),
            ("disabledQubitRewiringSupported", hardware),
            ("supportsUnassignedMeasurements", !hardware),
            ("supportsPartialVerbatimBox", seed.verbatim),
        ] {
            entry.insert(property.to_string(), serde_json::json!(value));
        }
    }
    serde_json::Value::Object(entry)
}

/// `supportedResultTypes`: one entry per result type the device computes, each with
/// the observables it may be taken against and the shot range it needs — none for an
/// exactly computed result type, the device's own range for a sampled one.
fn supported_result_types(seed: &DeviceSeed) -> serde_json::Value {
    serde_json::Value::Array(
        seed.result_types
            .iter()
            .map(|result_type| {
                let (min_shots, max_shots) = if result_type.exact {
                    (0, 0)
                } else {
                    (1.max(seed.shots_range.0), seed.shots_range.1)
                };
                let mut entry = serde_json::Map::new();
                entry.insert("name".to_string(), serde_json::json!(result_type.name));
                if !result_type.observables.is_empty() {
                    entry.insert(
                        "observables".to_string(),
                        serde_json::json!(result_type.observables),
                    );
                }
                entry.insert("minShots".to_string(), serde_json::json!(min_shots));
                entry.insert("maxShots".to_string(), serde_json::json!(max_shots));
                serde_json::Value::Object(entry)
            })
            .collect(),
    )
}

/// The pragmas an OpenQASM program for this device may and may not carry, as
/// (`supportedPragmas`, `forbiddenPragmas`).
///
/// Every pragma the service knows about appears in exactly one of the two lists, so a
/// client can tell "this device refuses it" from "this device has never heard of it":
/// a `braket_result_type_*` pragma is supported exactly where the device publishes
/// the matching result type, the [`NOISE_PRAGMAS`] exactly where it models noise,
/// `verbatim` exactly where it has a native gate layer to address, and
/// `braket_unitary_matrix` exactly where it accepts the `unitary` operation.
fn pragmas(seed: &DeviceSeed) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut supported: Vec<&'static str> = Vec::new();
    let mut forbidden: Vec<&'static str> = Vec::new();
    let mut sort = |pragma: &'static str, allowed: bool| {
        if allowed {
            supported.push(pragma);
        } else {
            forbidden.push(pragma);
        }
    };
    sort(VERBATIM_PRAGMA, seed.verbatim);
    sort(
        UNITARY_MATRIX_PRAGMA,
        seed.supported_operations.contains(&"unitary"),
    );
    for pragma in NOISE_PRAGMAS {
        sort(pragma, seed.noise);
    }
    for (result_type, pragma) in RESULT_TYPE_PRAGMAS {
        let published = seed
            .result_types
            .iter()
            .any(|seeded| seeded.name == *result_type);
        sort(pragma, published);
    }
    (supported, forbidden)
}

/// The device's own computing properties — which schema's they are, the qubits it
/// offers and, for a device with a native gate layer, the gates it executes without
/// recompilation. A simulator and an analog device have none, so they publish none.
fn device_properties(seed: &DeviceSeed) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "braketSchemaHeader".to_string(),
        serde_json::json!({ "name": device_properties_schema(seed), "version": "1" }),
    );
    properties.insert("qubitCount".to_string(), serde_json::json!(seed.qubit_count));
    if !seed.native_gates.is_empty() {
        properties.insert(
            "nativeGateSet".to_string(),
            serde_json::json!(seed.native_gates),
        );
    }
    serde_json::Value::Object(properties)
}

/// An instant as the ISO-8601 `date-time` a JSON document carries.
#[must_use]
pub fn as_iso8601(at: SystemTime) -> String {
    to_date_time(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBraketBraketState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBraketBraketState>)
}

/// The region every ARN this request mints carries: the one in the request's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request is unsigned.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// ARN syntax
//
// Two of the ARN shapes this service takes as INPUT constrain their value with
// `@arnReference` alone: `QuantumTaskArn` and `DeviceArn` carry no `@pattern`, so
// the framework's boundary validator — which does enforce the `@pattern` that
// `JobArn`, `SpendingLimitArn` and `Association$arn` carry — has nothing to check
// for them, and a value that is not an ARN at all reaches the handler. The two
// helpers below are the syntax check those members are held to, so a malformed ARN
// is refused as invalid input instead of being keyed into the store and reported as
// a resource that does not exist.
// ---------------------------------------------------------------------------

/// The service namespace every Amazon Braket ARN carries: the model's `sigv4Name`,
/// and the literal its own ARN patterns spell (`^arn:aws[a-z\-]*:braket:...`).
pub const ARN_SERVICE: &str = "braket";

/// The resource type a quantum-task ARN names (`QuantumTaskResource`).
pub const ARN_RESOURCE_QUANTUM_TASK: &str = "quantum-task";

/// The resource type a device ARN names (`DeviceResource`).
pub const ARN_RESOURCE_DEVICE: &str = "device";

/// The category segment a published simulator's device ARN carries
/// (`...:device/quantum-simulator/<provider>/<name>`).
pub const ARN_DEVICE_CATEGORY_SIMULATOR: &str = "quantum-simulator";

/// The category segment a published quantum processor's device ARN carries
/// (`...:device/qpu/<provider>/<name>`).
pub const ARN_DEVICE_CATEGORY_QPU: &str = "qpu";

/// The resource type a hybrid-job ARN names (`JobResource`), and the prefix
/// `JobArn`'s own `@pattern` spells (`...:[0-9]{12}:job/.*$`).
pub const ARN_RESOURCE_JOB: &str = "job";

/// The resource type a spending-limit ARN names (`SpendingLimitResource`), and the
/// resource prefix `CreateSpendingLimit` mints its ARNs with.
pub const ARN_RESOURCE_SPENDING_LIMIT: &str = "spending-limit";

/// The `position` a queue-info structure reports for a resource that is not waiting in
/// its queue any more.
///
/// `QuantumTaskQueueInfo$position` and `HybridJobQueueInfo$position` are both required
/// strings, and the Amazon Braket SDK documents `position` as "The current position of
/// the quantum task in the queue. If the task is not in a queue, the position is
/// 'None'"; a number here would claim the resource is still waiting. One definition for
/// both queue-info structures, so the two cannot drift.
pub const QUEUE_POSITION_NOT_QUEUED: &str = "None";

/// Whether `value` is syntactically an Amazon Braket ARN naming `resource_type`.
///
/// The form required is the ARN grammar
/// `arn:<partition>:braket:<region>:<account>:<resource-type>/<id>`:
///
/// * six `:`-separated fields, the first literally `arn` — the resource field is the
///   remainder, so it may itself carry `:` and `/`;
/// * a partition of the `aws` family, as this model's own ARN patterns spell it
///   (`arn:aws[a-z\-]*`), which is what admits `aws-cn` and `aws-us-gov`;
/// * the [`ARN_SERVICE`] namespace;
/// * a region drawn from `[a-z0-9\-]` and a 12-digit account id, EITHER of which may
///   be empty: `BraketResourceArn`'s own pattern permits an empty region, and the
///   device ARNs Amazon Braket publishes (`arn:aws:braket:::device/...`) carry
///   neither, so an empty field is not what makes an ARN malformed;
/// * `resource_type`, followed by `/` and a non-empty resource id.
///
/// Whether a well-formed ARN names a resource this emulator actually holds is a
/// different question, answered by the store: that is the
/// `ResourceNotFoundException` case.
pub fn is_braket_arn(value: &str, resource_type: &str) -> bool {
    braket_arn_resource_id(value, resource_type).is_some()
}

/// Whether `value` is syntactically an Amazon Braket ARN naming a resource of SOME type:
/// the field grammar [`braket_arn_resource`] reads, plus a resource field that carries a
/// type and a non-empty id (`<resource-type>/<id>`).
///
/// What an operation keyed to one kind of resource holds its ARN member to is
/// [`is_braket_arn`]; this is the check an operation takes that is addressed to ANY
/// taggable Braket resource, where the ARN's own type segment is what says which store
/// the resource would be in. Whether the named resource exists is a different question,
/// answered by the store.
pub fn is_braket_resource_arn(value: &str) -> bool {
    braket_arn_resource(value)
        .and_then(|resource| resource.split_once('/'))
        .is_some_and(|(resource_type, id)| !resource_type.is_empty() && !id.is_empty())
}

/// The resource id a Braket ARN of `resource_type` carries — everything after
/// `<resource_type>/` — or `None` when `value` is not such an ARN.
///
/// The ONE reading of the ARN field grammar this module states, so [`is_braket_arn`] and
/// the readings of what a resource id holds ([`device_type_from_arn`]) cannot disagree
/// about where the resource field starts.
fn braket_arn_resource_id<'v>(value: &'v str, resource_type: &str) -> Option<&'v str> {
    braket_arn_resource(value)?
        .strip_prefix(resource_type)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|id| !id.is_empty())
}

/// The RESOURCE field of a Braket ARN — everything after the fifth `:` — or `None` when
/// `value`'s five leading fields are not those of an Amazon Braket ARN.
fn braket_arn_resource(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let partition_is_aws = partition
        .strip_prefix("aws")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
    let region_is_well_formed = region
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let account_is_well_formed =
        account.is_empty() || (account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit()));
    (partition_is_aws && service == ARN_SERVICE && region_is_well_formed && account_is_well_formed)
        .then_some(resource)
}

/// The `DeviceType` a published device ARN itself names, read from the category segment
/// of its resource (`device/<category>/<provider>/<name>`).
///
/// Amazon Braket publishes a device's kind IN its ARN — [`ARN_DEVICE_CATEGORY_QPU`] for a
/// quantum processor and [`ARN_DEVICE_CATEGORY_SIMULATOR`] for a managed simulator — so an
/// operation whose rule turns on the KIND of device a value names can answer it from the
/// ARN alone, without the device having to be one this emulator's own inventory publishes.
///
/// `None` when the value is not a Braket device ARN at all, when it does not carry the
/// whole published form (a category, a provider and a device name), or when its category is
/// neither of the two Amazon Braket publishes devices under: such a value names no device
/// of any kind.
#[must_use]
pub fn device_type_from_arn(device_arn: &str) -> Option<&'static str> {
    // The resource id comes from this module's ONE reading of the ARN field grammar, which
    // also settles that the value is a Braket device ARN at all.
    let named = braket_arn_resource_id(device_arn, ARN_RESOURCE_DEVICE)?;
    // A published device ARN names a category, a provider and a device: a value that stops
    // short of any of them names no device, whatever its category segment reads as.
    let (category, provider_and_name) = named.split_once('/')?;
    let (provider, name) = provider_and_name.split_once('/')?;
    if provider.is_empty() || name.is_empty() {
        return None;
    }
    match category {
        ARN_DEVICE_CATEGORY_QPU => Some(DEVICE_TYPE_QPU),
        ARN_DEVICE_CATEGORY_SIMULATOR => Some(DEVICE_TYPE_SIMULATOR),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The sentences this service's client errors are worded with.
//
// Amazon Braket words a client error PER OPERATION, and the recordings show it: the
// identical malformed-ARN input is answered "quantumTaskArn is invalid." by one
// operation and "quantumTaskArn is not valid." by another, and a refusal that names a
// resource the caller cannot resolve carries the recovery pointer that operation gives.
// So what lives here is the VOCABULARY — one function per form the service words, with
// the member, the value and the region taken from the request — and each operation names
// the form IT words, in its own module. Nothing derives one sentence for the whole
// service: a single-homed sentence cannot satisfy two operations that word the same
// class differently.
// ---------------------------------------------------------------------------

/// `"<member> is not valid."`
///
/// `CancelQuantumTask`, `DeleteSpendingLimit` and `UpdateSpendingLimit` word an
/// ARN-shaped member whose value is not an ARN of the resource it names this way, and it
/// is the form an operation takes when the service's own sentence for it is not recorded:
/// the member, the fixed clause, no grammar description and no echo of the value.
pub fn member_is_not_valid(member: &str) -> String {
    format!("{member} is not valid.")
}

/// `"<member> is invalid."` — the same class, as `GetQuantumTask` words it.
pub fn member_is_invalid(member: &str) -> String {
    format!("{member} is invalid.")
}

/// `"<member> parameter is not valid."` — the same class, as `GetJob` words it.
pub fn member_parameter_is_not_valid(member: &str) -> String {
    format!("{member} parameter is not valid.")
}

/// `"<member> <value> is not a valid ARN."` — the same class, as `CancelJob` words it:
/// the one form that states the rejected value, taken from the request.
pub fn member_value_is_not_a_valid_arn(member: &str, value: &str) -> String {
    format!("{member} {value} is not a valid ARN.")
}

/// The identifier Amazon Braket names a hybrid job by in its messages: the account the
/// job belongs to and the job's name, joined by `#`, as the `jobArn` carries them
/// (`arn:<partition>:braket:<region>:<account>:job/<name>`).
///
/// Built from the REQUEST, so a refusal naming a job follows the ARN the caller sent
/// rather than any one recorded message. A value the ARN grammar cannot be read out of
/// falls back to the ARN itself; the modelled `@pattern` on `JobArn` makes that
/// unreachable from the wire. ONE reading for every door that names a job it could not
/// resolve, so `GetJob` and `CancelJob` cannot disagree about what identifies one.
#[must_use]
pub fn job_identifier(job_arn: &str) -> String {
    let mut fields = job_arn.splitn(6, ':');
    let account = fields.nth(4);
    let name = fields
        .next()
        .and_then(|resource| resource.strip_prefix(ARN_RESOURCE_JOB)?.strip_prefix('/'));
    match (account, name) {
        (Some(account), Some(name)) => format!("{account}#{name}"),
        _ => job_arn.to_string(),
    }
}

/// How an operation that resolves a device ARN against the published inventory and models
/// no not-found error refuses an ARN naming no published device.
///
/// Fixed text interpolating nothing — not even the ARN that failed to resolve — and it
/// points at the operation that enumerates the inventory, which is the only recovery a
/// caller has for a device ARN the service does not publish.
pub const DEVICE_ARN_NOT_FOUND: &str =
    "Device ARN not found. Please use SearchDevices operation to get a list of supported \
     devices.";

/// How a door that holds a request's `shots` to the device's own published range refuses
/// one outside it: the bounds the device publishes, the device the request named, and the
/// count it asked for — every part taken from the request and from the device's capability
/// document.
#[must_use]
pub fn shots_out_of_range(min: i64, max: i64, device_arn: &str, shots: i64) -> String {
    format!("Shot count should be between {min} and {max} on {device_arn}, but found {shots}.")
}

/// How this service reports a member of a LIST in the request whose value is not one the
/// member allows: the per-member validation payload Amazon Braket answers with, rendered
/// from the rejected member path and value.
///
/// The payload names the list member, the index of the element that failed, the member of
/// that element that failed, and the value that was not allowed — so a client parsing it
/// can point at the field it sent, which is what makes the message request-derived rather
/// than one fixed sentence per operation:
/// `{'filters': [{0: [{'name': ['unallowed value notAFilter']}]}]}`.
#[must_use]
pub fn unallowed_member_value(list_member: &str, index: usize, member: &str, value: &str) -> String {
    format!("{{'{list_member}': [{{{index}: [{{'{member}': ['unallowed value {value}']}}]}}]}}")
}

/// How this service reports a member of a list element whose value is outside the bound
/// the member carries, in the same per-member form as [`unallowed_member_value`]: the
/// rejected member path and what the value failed.
#[must_use]
pub fn member_value_refusal(
    list_member: &str,
    index: usize,
    member: &str,
    failure: &str,
) -> String {
    format!("{{'{list_member}': [{{{index}: [{{'{member}': ['{failure}']}}]}}]}}")
}

/// How every door that refuses a device because the inventory publishes it as `RETIRED`
/// words that refusal.
///
/// The recorded behaviour of the `DeviceRetiredException` class is one FIXED sentence
/// naming the status that closed the device and the recovery left to the caller: it
/// interpolates nothing from the request — not the ARN, not the device name — so the
/// class is reported identically whatever device was asked for, and stays distinguishable
/// from the `ValidationException` classes that refuse a device for what KIND it is.
pub const DEVICE_RETIRED_REFUSAL: &str = "Device is RETIRED. Try another device.";

/// The service principal a role Amazon Braket acts under has to trust: Braket assumes the
/// `roleArn` a hybrid job is created with, so a role whose trust policy does not admit this
/// principal is one the service cannot use.
pub const BRAKET_SERVICE_PRINCIPAL: &str = "braket.amazonaws.com";

/// How an operation that resolves a caller-supplied execution role refuses one the service
/// cannot assume: the fixed sentence, with the `roleArn` the REQUEST carried quoted in it
/// and nothing else interpolated.
#[must_use]
pub fn cannot_assume_role(role_arn: &str) -> String {
    format!("Braket cannot assume the IAM role: '{role_arn}'")
}

/// How `GetDevice`, which does model a not-found error, reports an ARN the inventory does
/// not publish: the ARN the request named and the region the request was signed for, which
/// is how a caller learns a device it cannot see here may be published in another region,
/// then the developer guide's device list.
pub fn device_not_found_in_region(device_arn: &str, region: &str) -> String {
    format!(
        "Braket device '{device_arn}' not found in {region}. You can find a list of all \
         supported device ARNs and the regions in which they are available in the \
         documentation: {DOCUMENTATION_BASE}/{DEVICE_LIST_PAGE}"
    )
}

/// How a door that takes a `TimePeriod` refuses one that covers no time at all, or
/// `None` when the window is well formed.
///
/// `TimePeriod` is a window with a required start and a required end
/// (`smithy_query(query="shape", shape="TimePeriod")`), so a period whose end does not
/// follow its start is no window: there is no span for anything to be active over. The
/// rule holds wherever this service takes a `TimePeriod`, so every door that takes one
/// words it from here, with the two instants the REQUEST carried.
#[must_use]
pub fn time_period_refusal(period: &TimePeriod) -> Option<String> {
    let start = date_time_as_nanos(&period.start_at);
    let end = date_time_as_nanos(&period.end_at);
    (end <= start).then(|| {
        format!(
            "timePeriod is invalid: endAt ({}) must be after startAt ({})",
            period.end_at.secs(),
            period.start_at.secs()
        )
    })
}

/// How fine an instant this service records is held, in nanoseconds: ONE MICROSECOND
/// ([`INSTANT_RESOLUTION_NANOS`]).
///
/// Every `Timestamp` this service reports travels in the `date-time` form its model
/// declares, and that form carries a whole microsecond: a rendering is read back as the
/// same instant only up to that digit. An instant held FINER than the form it is served
/// in is therefore an instant no reader can repeat — the value a caller read back names
/// no record of this service, and a second service publishing it republishes a different
/// rendering of it. So the remainder below a microsecond is dropped when the instant is
/// recorded rather than when it is written, which is what makes one instant render
/// identically on every path that reports it.
const INSTANT_RESOLUTION_NANOS: u32 = 1_000;

/// A `SystemTime` as the modelled `Timestamp` the wire carries, held at the resolution the
/// form it travels in carries ([`INSTANT_RESOLUTION_NANOS`]).
///
/// The conversion is deterministic — one instant always renders to the same `Timestamp` —
/// so a value read twice is the same value, and a value read back is the value this
/// service compares a filter bound against.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX);
    let nanos = since_epoch.subsec_nanos() / INSTANT_RESOLUTION_NANOS * INSTANT_RESOLUTION_NANOS;
    ::aws_smithy_types::DateTime::from_secs_and_nanos(seconds, nanos)
}

/// A modelled `Timestamp` as whole nanoseconds since the epoch, so two `Timestamp`s
/// are compared on one integer scale rather than field by field.
pub fn date_time_as_nanos(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * 1_000_000_000 + i128::from(at.subsec_nanos())
}

/// A stable hex digest of `parts`, used to tell one request from another (an
/// idempotency replay from a reuse of the same token, a pagination token from one
/// issued for a different filter set).
///
/// Each part is length-prefixed, so no two different field lists can render to the
/// same input.
pub fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

/// Mint the identifier of a new quantum task: a version-4 UUID, as the resource part
/// of a Braket quantum-task ARN carries.
///
/// Built here rather than with the `uuid` crate for one reason: adding a dependency
/// adds this crate's `Cargo.lock` entry to the change, and that one line is what the
/// serialized merge into the shared mainline kept refusing to squash. See
/// [`new_resource_id`] for how the bytes are produced.
pub fn new_task_id() -> String {
    new_resource_id("braket-quantum-task")
}

/// Mint the identifier of a new spending limit: a version-4 UUID, as the resource part
/// of a Braket spending-limit ARN carries. See [`new_resource_id`].
pub fn new_spending_limit_id() -> String {
    new_resource_id("braket-spending-limit")
}

/// Mint a version-4 UUID for a new resource of kind `domain`.
///
/// The bytes come from a SHA-256 over values unique to this call — the resource kind,
/// a per-process counter and the wall clock — so no two resources share an id and the
/// id is not derived from the request; the version and variant nibbles are then set,
/// so the result is a well-formed v4 UUID (RFC 9562 section 5.4: 122 random bits,
/// version 0b0100, variant 0b10).
fn new_resource_id(domain: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(seq.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&bytes[0..4]),
        hex::encode(&bytes[4..6]),
        hex::encode(&bytes[6..8]),
        hex::encode(&bytes[8..10]),
        hex::encode(&bytes[10..16]),
    )
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// The largest page either search operation returns, and the page size it uses when
/// the caller names none. It is the `maxResults` maximum the model declares
/// (`@range(min: 1, max: 100)`).
pub const MAX_PAGE_SIZE: usize = 100;

/// The most filters one search request may carry.
///
/// The model's own constraint on both filter lists
/// (`SearchDevicesRequest$filters` / `SearchQuantumTasksRequest$filters`,
/// `@length(min: 0, max: 10)`), so the boundary validator refuses a request past it
/// before a handler runs. Each handler re-checks it so the work it does is bounded by
/// the code a reader can see, not only by the schema: a search performs at most
/// `MAX_FILTERS_PER_REQUEST * MAX_VALUES_PER_FILTER * MAX_QUANTUM_TASKS` comparisons.
pub const MAX_FILTERS_PER_REQUEST: usize = 10;

/// The most values one filter may carry: the model's own constraint on both filters'
/// `values` member (`String256List`, `@length(min: 1, max: 10)`).
pub const MAX_VALUES_PER_FILTER: usize = 10;

/// Where a page of results starts, and which filter set it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// How many matches precede this page.
    pub offset: usize,
    /// The digest of the filters the token was issued for, so a token replayed
    /// against a different filter set is refused instead of silently skipping.
    pub fingerprint: String,
}

impl PageCursor {
    /// Render the cursor as the opaque `nextToken` a caller echoes back. The
    /// encoding carries the cursor itself, so issuing a token writes no state —
    /// which is what lets the `@readonly` search operations hand one out.
    pub fn encode(&self) -> String {
        hex::encode(format!("v1:{}:{}", self.offset, self.fingerprint))
    }

    /// Read back a token this service issued. `None` for anything else.
    pub fn decode(token: &str) -> Option<Self> {
        let decoded = hex::decode(token).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let mut fields = text.split(':');
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some("v1"), Some(offset), Some(fingerprint), None) => Some(Self {
                offset: offset.parse().ok()?,
                fingerprint: fingerprint.to_string(),
            }),
            _ => None,
        }
    }
}

/// Take the page of `items` the cursor and page size select, and the token that
/// returns the next one.
///
/// `None` for the token on the last page: the paginated contract ends a traversal
/// by omitting `nextToken`.
pub fn paginate<T: Clone>(
    items: &[T],
    offset: usize,
    page_size: usize,
    fingerprint: &str,
) -> (Vec<T>, Option<String>) {
    let start = offset.min(items.len());
    let end = start.saturating_add(page_size).min(items.len());
    let page = items[start..end].to_vec();
    let next = (end < items.len()).then(|| {
        PageCursor {
            offset: end,
            fingerprint: fingerprint.to_string(),
        }
        .encode()
    });
    (page, next)
}
