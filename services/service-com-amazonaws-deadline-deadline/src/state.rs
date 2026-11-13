// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-deadline-deadline service.
//!
//! The resource hierarchy this service persists is `farm -> queue -> job`, exactly as
//! the model's paths nest it (`/2023-10-12/farms/{farmId}/queues/{queueId}/jobs/{jobId}`),
//! so a queue is reachable only through its farm and a job only through its queue. A
//! farm's budgets hang off it the same way
//! (`/2023-10-12/farms/{farmId}/budgets/{budgetId}`). Every handler reads and writes
//! through [`service_state`]; nothing here is per-request.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

/// `QueueStatus` value a freshly created queue carries.
///
/// From `smithy_query(query="shape", shape="QueueStatus")`: `IDLE`, `SCHEDULING`,
/// `SCHEDULING_BLOCKED`. A queue with no fleet associated to it is scheduling nothing,
/// so it stays `IDLE` until something schedules on it.
pub const QUEUE_STATUS_IDLE: &str = "IDLE";

/// `FleetStatus` a fleet carries while the service is still standing it up.
///
/// From `smithy_query(query="shape", shape="FleetStatus")`: `ACTIVE`,
/// `CREATE_IN_PROGRESS`, `UPDATE_IN_PROGRESS`, `CREATE_FAILED`, `UPDATE_FAILED`,
/// `SUSPENDED`.
pub const FLEET_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `FleetStatus` a fleet reaches once the service has stood it up.
pub const FLEET_STATUS_ACTIVE: &str = "ACTIVE";

/// How long after `CreateFleet` the fleet's `CREATE_IN_PROGRESS -> ACTIVE` transition
/// comes due.
///
/// `CreateFleet` answers with a fleet in a NON-TERMINAL status, so reaching `ACTIVE` is
/// owed by the clock rather than by any later call. AWS documents no duration for it, so
/// this is the emulator's chosen span: a caller that reads the fleet straight back still
/// observes `CREATE_IN_PROGRESS`, and nothing ever sleeps for it — the due instant is
/// stored and compared on read.
pub const FLEET_ACTIVE_AFTER: Duration = Duration::from_secs(3);

/// `AutoScalingStatus` a fleet carries while it is below its target worker count.
///
/// From `smithy_query(query="shape", shape="AutoScalingStatus")`: `GROWING`, `STEADY`,
/// `SHRINKING`.
pub const AUTO_SCALING_STATUS_GROWING: &str = "GROWING";
/// `AutoScalingStatus` a fleet carries while it holds exactly its target worker count.
pub const AUTO_SCALING_STATUS_STEADY: &str = "STEADY";
/// `AutoScalingStatus` a fleet carries while it holds more workers than its target.
pub const AUTO_SCALING_STATUS_SHRINKING: &str = "SHRINKING";

/// `WorkerStatus` a worker carries from the moment it is registered.
///
/// From `smithy_query(query="shape", shape="WorkerStatus")`: `CREATED`, `STARTED`,
/// `STOPPING`, `STOPPED`, `NOT_RESPONDING`, `NOT_COMPATIBLE`, `RUNNING`, `IDLE`. A worker
/// is the agent's own registration, and only the agent reports it onward
/// (`UpdatedWorkerStatus` is `STARTED` | `STOPPING` | `STOPPED`), so a registered worker
/// stays here until a call moves it: nothing about this status is owed by the clock.
pub const WORKER_STATUS_CREATED: &str = "CREATED";

/// `JobLifecycleStatus` a job carries while the service is still assembling it.
///
/// From `smithy_query(query="shape", shape="JobLifecycleStatus")`.
pub const JOB_LIFECYCLE_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `JobLifecycleStatus` a job reaches once the service has assembled it.
pub const JOB_LIFECYCLE_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long after `CreateJob` the job's `CREATE_IN_PROGRESS -> CREATE_COMPLETE`
/// transition comes due.
///
/// `CreateJob` returns a job in a NON-TERMINAL lifecycle status, so the move to
/// `CREATE_COMPLETE` is owed by the clock rather than by any later call. AWS documents no
/// duration for it, so this is the emulator's chosen span inside the platform's
/// 250–750 ms band: a caller that reads the job straight back still observes
/// `CREATE_IN_PROGRESS`, and nothing ever sleeps for it — the due instant is stored and
/// compared on read.
pub const JOB_CREATE_COMPLETE_AFTER: Duration = Duration::from_millis(500);

/// `JobLifecycleStatus` a job reaches once the service has reconciled the created job
/// against what its queue can schedule onto.
pub const JOB_LIFECYCLE_UPDATE_SUCCEEDED: &str = "UPDATE_SUCCEEDED";

/// `TaskRunStatus` a job carries while the service is still assembling it: the job exists
/// but none of its tasks can be scheduled yet.
///
/// From `smithy_query(query="shape", shape="TaskRunStatus")`: `PENDING`, `READY`,
/// `ASSIGNED`, `STARTING`, `SCHEDULED`, `INTERRUPTING`, `RUNNING`, `SUSPENDED`,
/// `CANCELED`, `FAILED`, `SUCCEEDED`, `NOT_COMPATIBLE`.
pub const TASK_RUN_STATUS_PENDING: &str = "PENDING";

/// `TaskRunStatus` of a job whose tasks the service is free to hand to a worker.
pub const TASK_RUN_STATUS_READY: &str = "READY";

/// `TaskRunStatus` a job reaches when no fleet that could run its tasks is associated with
/// its queue, so nothing can ever pick it up.
pub const TASK_RUN_STATUS_NOT_COMPATIBLE: &str = "NOT_COMPATIBLE";

/// How long after `CreateJob` the service's reconciliation of the job against the fleets
/// its queue schedules onto comes due.
///
/// A created job is not where it ends up: the service places it against the queue's fleets
/// after answering, so that move is owed by the clock rather than by any later call. AWS
/// documents no duration for it, so this is the emulator's chosen span. It is the long one
/// of this service's spans on purpose: a created job has to stay OBSERVABLE at the
/// lifecycle status its creation settled it to — the status callers poll a fresh job for —
/// for long enough that a client reading it every few seconds sees that status before this
/// move takes the job past it. Nothing ever sleeps for it: the due instant is stored and
/// compared on read.
pub const JOB_RECONCILE_AFTER: Duration = Duration::from_secs(15);

/// `QueueFleetAssociationStatus` a freshly created queue-fleet association carries.
///
/// From `smithy_query(query="shape", shape="QueueFleetAssociationStatus")`: `ACTIVE`,
/// `STOPPED`, `STOP_SCHEDULING_AND_CANCEL_TASKS`, `STOP_SCHEDULING_AND_COMPLETE_TASKS`.
/// An association is created scheduling the queue's work onto the fleet, and only
/// `UpdateQueueFleetAssociation` moves it out of that status, so nothing here is owed by
/// the clock.
pub const QUEUE_FLEET_ASSOCIATION_STATUS_ACTIVE: &str = "ACTIVE";

/// How long after `UpdateQueueFleetAssociation` asks an association to stop scheduling the
/// queue's work the `STOP_SCHEDULING_AND_* -> STOPPED` move comes due.
///
/// Those two statuses are the association DRAINING: the update answers with it in a
/// non-terminal status, so reaching [`QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED`] is owed by
/// the clock rather than by any later call. AWS documents no duration for it, so this is
/// the emulator's chosen span: a caller that reads the association straight back still
/// observes it draining — and a delete asked for then is still refused — and nothing ever
/// sleeps for it, because the due instant is stored and compared on read.
pub const QUEUE_FLEET_ASSOCIATION_STOPPED_AFTER: Duration = Duration::from_secs(3);

/// The `QueueFleetAssociationStatus` an association has stopped scheduling the queue's
/// work onto the fleet in.
///
/// The one TERMINAL value of that enum: the two `STOP_SCHEDULING_AND_*` statuses are the
/// association on its way here, and `ACTIVE` is it scheduling. An association is detached
/// from the fleet's work only in this status, which is what makes it the one status a
/// delete of the association is allowed in.
pub const QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED: &str = "STOPPED";

/// `QueueLimitAssociationStatus` a freshly created queue-limit association carries.
///
/// From `smithy_query(query="shape", shape="QueueLimitAssociationStatus")`: `ACTIVE`,
/// `STOP_LIMIT_USAGE_AND_COMPLETE_TASKS`, `STOP_LIMIT_USAGE_AND_CANCEL_TASKS`, `STOPPED`.
/// The model documents `ACTIVE` as "Association is active", and the association subjects
/// the queue's work to the limit from the moment it exists, so a create answers with it in
/// this status; only `UpdateQueueLimitAssociation` asks it to stop, so nothing here is owed
/// by the clock.
pub const QUEUE_LIMIT_ASSOCIATION_STATUS_ACTIVE: &str = "ACTIVE";

/// `BudgetStatus` a freshly created budget carries.
///
/// From `smithy_query(query="shape", shape="BudgetStatus")`: `ACTIVE`, `INACTIVE`. A
/// budget is evaluated from the moment it exists, so a create answers with a budget that
/// is already in this TERMINAL status — nothing is owed by the clock.
pub const BUDGET_STATUS_ACTIVE: &str = "ACTIVE";

/// `BudgetStatus` a budget reaches when it stops being evaluated.
///
/// Terminal in the other direction too: once a budget is here it stays here, so
/// [`Budget::reactivation_refused`] refuses a move back to [`BUDGET_STATUS_ACTIVE`].
pub const BUDGET_STATUS_INACTIVE: &str = "INACTIVE";

/// `resourceType` this service reports for a farm in `ResourceNotFoundException`.
pub const RESOURCE_TYPE_FARM: &str = "farm";
/// `resourceType` this service reports for a queue in `ResourceNotFoundException`.
pub const RESOURCE_TYPE_QUEUE: &str = "queue";
/// `resourceType` this service reports for a fleet in `ResourceNotFoundException`.
pub const RESOURCE_TYPE_FLEET: &str = "fleet";
/// `resourceType` this service reports for a worker in `ResourceNotFoundException` and
/// `ConflictException`.
pub const RESOURCE_TYPE_WORKER: &str = "worker";
/// `resourceType` this service reports for a job in `ResourceNotFoundException`.
pub const RESOURCE_TYPE_JOB: &str = "job";
/// `resourceType` this service reports for a queue-fleet association in
/// `ResourceNotFoundException`. The model documents the member only as "the type of the
/// resource that couldn't be found" and enumerates no values, so a resource type is
/// spelled the way the API path spells that resource — the segment
/// `/queue-fleet-associations/` names it `queue-fleet-association`.
pub const RESOURCE_TYPE_QUEUE_FLEET_ASSOCIATION: &str = "queue-fleet-association";
/// `resourceType` this service reports for a queue-limit association in
/// `ResourceNotFoundException`. Spelled the way the API path spells that resource, like
/// every other type here — the segment `/queue-limit-associations/` names it
/// `queue-limit-association`.
pub const RESOURCE_TYPE_QUEUE_LIMIT_ASSOCIATION: &str = "queue-limit-association";
/// `resourceType` this service reports for a budget in `ResourceNotFoundException`.
pub const RESOURCE_TYPE_BUDGET: &str = "budget";
/// `resourceType` this service reports for a limit in `ResourceNotFoundException`. The
/// model documents the member only as "the type of the resource that couldn't be found"
/// and enumerates no values, so a resource type is spelled the way the API path spells
/// that resource — the segment `/limits/` names it `limit`.
pub const RESOURCE_TYPE_LIMIT: &str = "limit";
/// `resourceType` this service reports for a storage profile in
/// `ResourceNotFoundException`. The model documents the member only as "the type of the
/// resource that couldn't be found" and enumerates no values, so a resource type is
/// spelled the way the API path spells that resource — the segment `/storage-profiles/`
/// names it `storage-profile`, kebab-case and singular like every other type above.
pub const RESOURCE_TYPE_STORAGE_PROFILE: &str = "storage-profile";

/// How many client tokens the store remembers at once.
///
/// A client token is a retry-deduplication record, not a resource: it must not let a
/// caller grow the store without bound, so the map is capped and the oldest entry is
/// evicted past the cap (and a delete forgets the tokens of what it removed).
pub const MAX_REMEMBERED_CLIENT_TOKENS: usize = 4096;

/// How many farms this emulator holds at once.
///
/// Every create is one caller-driven insert into a store that only an explicit delete
/// empties, so each level of the hierarchy carries a maximum beyond which the create is
/// refused with the modeled `ServiceQuotaExceededException` (HTTP 402). These are the
/// EMULATOR's memory bounds, not a published AWS quota: AWS's own farm/queue/job limits
/// are account-adjustable and are not in the model, so a number here would be invented.
/// They are set far above what any client workflow needs and below what would exhaust a
/// process.
pub const MAX_FARMS: usize = 500;

/// How many queues one farm holds at once. See [`MAX_FARMS`].
pub const MAX_QUEUES_PER_FARM: usize = 500;

/// How many jobs one queue holds at once. See [`MAX_FARMS`].
pub const MAX_JOBS_PER_QUEUE: usize = 1000;

/// How many fleets one farm holds at once. See [`MAX_FARMS`].
pub const MAX_FLEETS_PER_FARM: usize = 500;

/// How many workers this emulator stores for one fleet.
///
/// A fleet's own `maxWorkerCount` is the service's limit on its worker count, but it is a
/// caller-chosen `MinZeroMaxInteger`, so it bounds nothing this process has to hold: this
/// is the store's own ceiling on an insert only a delete releases, alongside the other
/// per-parent bounds here.
pub const MAX_WORKERS_PER_FLEET: usize = 500;

/// How many budgets one farm holds at once. See [`MAX_FARMS`].
pub const MAX_BUDGETS_PER_FARM: usize = 500;

/// How many storage profiles one farm holds at once. See [`MAX_FARMS`].
pub const MAX_STORAGE_PROFILES_PER_FARM: usize = 500;

/// How many limits one farm holds at once. See [`MAX_FARMS`].
pub const MAX_LIMITS_PER_FARM: usize = 500;

/// The `maxCount` a limit may not carry.
///
/// `smithy_query(query="doc", shape="CreateLimitRequest", member="maxCount")` states it:
/// "The `maxCount` must not be 0. If the value is -1, there is no restriction on the
/// number of resources that can be acquired for this limit." The modeled `@range` is
/// `-1..2147483647`, which spans 0, so the boundary validator passes it and the handler is
/// what refuses it.
pub const FORBIDDEN_MAX_COUNT: i32 = 0;

/// How many bytes of job templates this emulator stores at once.
///
/// A job template is the one request member that can be a megabyte on its own
/// (`JobTemplate, len 1..1000000`), and `sourceJobId` lets a small request store another
/// copy of one, so the templates are budgeted by BYTE as well as by count. The budget is
/// released when the jobs holding those bytes are deleted.
pub const MAX_STORED_TEMPLATE_BYTES: usize = 8 * 1024 * 1024;

/// `quotaCode` this service reports when a create would pass one of those bounds.
pub const QUOTA_CODE_RESOURCE_COUNT: &str = "L-RESOURCE-COUNT";

/// The largest page any paginated read of this service returns, and the modeled default.
///
/// From `smithy_query(query="operation", operation="ListFarms")`: `maxResults` is
/// `range 1..100, default 100`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// A persisted farm, as `CreateFarm` stored it.
#[derive(Debug, Clone)]
pub struct Farm {
    pub farm_id: String,
    pub display_name: String,
    pub description: String,
    pub kms_key_arn: Option<String>,
    pub cost_scale_factor: f32,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    pub queues: HashMap<String, Queue>,
    pub fleets: HashMap<String, Fleet>,
    /// Every budget of this farm, keyed by `budgetId`. A budget is reached only through
    /// its farm's path (`/2023-10-12/farms/{farmId}/budgets/{budgetId}`), so it hangs off
    /// the farm exactly as a queue does.
    pub budgets: HashMap<String, Budget>,
    /// Every storage profile of this farm, keyed by `storageProfileId`. A storage profile
    /// is reached only through its farm's path
    /// (`/2023-10-12/farms/{farmId}/storage-profiles/{storageProfileId}`), so it hangs off
    /// the farm exactly as a queue does.
    pub storage_profiles: HashMap<String, StorageProfile>,
    /// Every limit of this farm, keyed by `limitId`. A limit is reached only through its
    /// farm's path (`/2023-10-12/farms/{farmId}/limits/{limitId}`), so it hangs off the
    /// farm exactly as a queue does.
    pub limits: HashMap<String, Limit>,
}

/// A persisted limit, as `CreateLimit` stored it under its farm.
///
/// A limit is a counter of shared resources — it is created holding everything it will
/// ever be and carries no status, so the create answers with a limit that is already
/// complete and nothing is owed by the clock.
#[derive(Debug, Clone)]
pub struct Limit {
    pub limit_id: String,
    pub farm_id: String,
    pub display_name: String,
    pub description: String,
    pub amount_requirement_name: String,
    pub max_count: i32,
    /// How many of the limit's resources are in use by jobs.
    ///
    /// A limit is created with none of its resources acquired, and it stays at none until
    /// something in this emulator schedules work that holds the limit: nothing assigns a
    /// session to a step, so no job has acquired a resource from it.
    pub current_count: i32,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
}

/// A persisted storage profile, as `CreateStorageProfile` stored it under its farm.
///
/// A storage profile is a description of where resources live — an operating-system
/// family and a list of file system locations — so it carries no status: the create
/// answers with a profile that is already everything it will ever be, and nothing is
/// owed by the clock.
#[derive(Debug, Clone)]
pub struct StorageProfile {
    pub storage_profile_id: String,
    pub farm_id: String,
    pub display_name: String,
    pub os_family: String,
    pub file_system_locations: Option<Vec<crate::ops::types::FileSystemLocation>>,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
}

/// A persisted fleet, as `CreateFleet` stored it under its farm.
#[derive(Debug, Clone)]
pub struct Fleet {
    pub fleet_id: String,
    pub farm_id: String,
    pub display_name: String,
    pub description: String,
    pub role_arn: String,
    pub min_worker_count: i32,
    pub max_worker_count: i32,
    pub configuration: crate::ops::types::FleetConfiguration,
    pub host_configuration: Option<crate::ops::types::HostConfiguration>,
    pub status: String,
    /// The workers registered with this fleet, keyed by `workerId`, exactly as the model's
    /// paths nest them (`/2023-10-12/farms/{farmId}/fleets/{fleetId}/workers/{workerId}`),
    /// so a worker is reachable only through its own fleet and its count has one source.
    pub workers: HashMap<String, Worker>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    /// When the clock-owed `CREATE_IN_PROGRESS -> ACTIVE` move comes due; `None` once it
    /// has been materialised. See [`FLEET_ACTIVE_AFTER`].
    pub active_due_at: Option<Instant>,
}

impl Fleet {
    /// Materialise the status transition the clock already made due.
    ///
    /// A read calls this before answering: the move was owed at this instant whether or
    /// not anybody called, so applying it on read is reporting, not mutating.
    pub fn settle_status(&mut self) {
        if let Some(due) = self.active_due_at {
            if Instant::now() >= due {
                self.status = FLEET_STATUS_ACTIVE.to_string();
                self.active_due_at = None;
            }
        }
    }

    /// How many workers are registered with the fleet.
    ///
    /// Derived from the workers the fleet holds rather than counted alongside them, so a
    /// create or a delete cannot leave the reported count and the stored workers
    /// disagreeing.
    pub fn worker_count(&self) -> i32 {
        i32::try_from(self.workers.len()).unwrap_or(i32::MAX)
    }

    /// How many workers the fleet is scaling towards.
    ///
    /// Nothing schedules work onto a fleet in this emulator, so the only pull on its
    /// worker count is the floor its own configuration sets: the target is the fleet's
    /// minimum, never above its maximum.
    pub fn target_worker_count(&self) -> i32 {
        self.min_worker_count.min(self.max_worker_count)
    }

    /// Where the fleet's worker count stands relative to [`target_worker_count`].
    ///
    /// [`target_worker_count`]: Self::target_worker_count
    pub fn auto_scaling_status(&self) -> &'static str {
        match self.worker_count().cmp(&self.target_worker_count()) {
            ::std::cmp::Ordering::Less => AUTO_SCALING_STATUS_GROWING,
            ::std::cmp::Ordering::Equal => AUTO_SCALING_STATUS_STEADY,
            ::std::cmp::Ordering::Greater => AUTO_SCALING_STATUS_SHRINKING,
        }
    }

    /// What the fleet is capable of, as its configuration declares it.
    ///
    /// Only the capabilities the configuration NAMES are reported: the custom amounts and
    /// attributes the fleet's worker capabilities carry. The model defines no names for
    /// the built-in vCPU, memory, OS and architecture capabilities, so a fleet whose
    /// configuration declares no custom capability reports none rather than a name this
    /// emulator made up; `configuration` reports those built-in ranges verbatim.
    /// `FleetAmountCapabilities`/`FleetAttributeCapabilities` are `len 1..25`, so an empty
    /// list is left out instead of sent empty.
    pub fn capabilities(&self) -> Option<crate::ops::types::FleetCapabilities> {
        let (amounts, attributes) = match &self.configuration {
            crate::ops::types::FleetConfiguration::customerManaged(managed) => (
                managed.worker_capabilities.custom_amounts.clone(),
                managed.worker_capabilities.custom_attributes.clone(),
            ),
            crate::ops::types::FleetConfiguration::serviceManagedEc2(managed) => (
                managed.instance_capabilities.custom_amounts.clone(),
                managed.instance_capabilities.custom_attributes.clone(),
            ),
        };
        let amounts = amounts.filter(|amounts| !amounts.is_empty());
        let attributes = attributes.filter(|attributes| !attributes.is_empty());
        if amounts.is_none() && attributes.is_none() {
            return None;
        }
        Some(crate::ops::types::FleetCapabilities {
            amounts,
            attributes,
        })
    }
}

/// Every lower/upper capability pair a fleet configuration declares, as
/// `(member, min, max)`, where `member` is the pair's own path in the request.
///
/// The model declares one `min`/`max` structure per capability range
/// (`smithy_query(query="closure", operation="CreateFleet")`: `VCpuCountRange`,
/// `MemoryMiBRange`, `AcceleratorCountRange`, `AcceleratorTotalMemoryMiBRange`), plus the
/// same pair on every `FleetAmountCapability` the configuration's custom amounts carry.
/// Only `min` is required in each, so a pair with no upper bound is open-ended rather than
/// inverted. The bounds are reported as `f64` because an amount capability's are floats
/// while the counts are integers.
///
/// Shared by every operation that takes a `FleetConfiguration` — the configuration a
/// create stores and the configuration an update replaces it with are the same shape, so
/// the pairs they are checked on come from one place.
pub fn fleet_capability_ranges(
    configuration: &crate::ops::types::FleetConfiguration,
) -> Vec<(String, f64, Option<f64>)> {
    use crate::ops::types::FleetConfiguration;

    let mut ranges: Vec<(String, f64, Option<f64>)> = Vec::new();
    // The path the request nests these capabilities under, so a refusal names the member
    // the caller sent rather than a bare structure name.
    let capabilities_path = match configuration {
        FleetConfiguration::customerManaged(_) => {
            "configuration.customerManaged.workerCapabilities"
        }
        FleetConfiguration::serviceManagedEc2(_) => {
            "configuration.serviceManagedEc2.instanceCapabilities"
        }
    };
    let mut push_count = |member: &str, min: i32, max: Option<i32>| {
        ranges.push((
            format!("{capabilities_path}.{member}"),
            f64::from(min),
            max.map(f64::from),
        ));
    };
    let custom_amounts = match configuration {
        FleetConfiguration::customerManaged(managed) => {
            let capabilities = &managed.worker_capabilities;
            push_count(
                "vCpuCount",
                capabilities.v_cpu_count.min,
                capabilities.v_cpu_count.max,
            );
            push_count(
                "memoryMiB",
                capabilities.memory_mi_b.min,
                capabilities.memory_mi_b.max,
            );
            if let Some(count) = &capabilities.accelerator_count {
                push_count("acceleratorCount", count.min, count.max);
            }
            if let Some(memory) = &capabilities.accelerator_total_memory_mi_b {
                push_count("acceleratorTotalMemoryMiB", memory.min, memory.max);
            }
            capabilities.custom_amounts.as_deref()
        }
        FleetConfiguration::serviceManagedEc2(managed) => {
            let capabilities = &managed.instance_capabilities;
            push_count(
                "vCpuCount",
                capabilities.v_cpu_count.min,
                capabilities.v_cpu_count.max,
            );
            push_count(
                "memoryMiB",
                capabilities.memory_mi_b.min,
                capabilities.memory_mi_b.max,
            );
            if let Some(count) = capabilities
                .accelerator_capabilities
                .as_ref()
                .and_then(|accelerators| accelerators.count.as_ref())
            {
                push_count("acceleratorCount", count.min, count.max);
            }
            capabilities.custom_amounts.as_deref()
        }
    };
    // A custom amount is one entry of a list, so its path carries that entry's index.
    for (index, amount) in custom_amounts.unwrap_or_default().iter().enumerate() {
        ranges.push((
            format!("{capabilities_path}.customAmounts[{index}]"),
            f64::from(amount.min),
            amount.max.map(f64::from),
        ));
    }
    ranges
}

/// The refusal a lower/upper pair of a fleet request earns when its lower bound is above
/// its upper bound.
///
/// Each bound satisfies its own modeled `@range` on its own — the relation BETWEEN them is
/// what no boundary validator can check — so this is the handler's refusal, naming the pair
/// at fault and the window the caller asked for. Shared by every operation that takes a
/// `FleetConfiguration`, so one inverted pair is worded one way wherever it arrives.
pub fn inverted_range(
    member: &str,
    min: f64,
    max: f64,
) -> crate::ops::types::ValidationException {
    field_validation_failed(
        member,
        &format!("{member}.min ({min}) must be less than or equal to {member}.max ({max})"),
    )
}

/// The refusal a fleet request earns when the worker-count window it asks for is not a
/// window.
///
/// A fleet's `minWorkerCount` is the floor the service scales it to and its
/// `maxWorkerCount` the ceiling it never passes, so a minimum above the maximum describes
/// no fleet. Both bounds satisfy `MinZeroMaxInteger` (`range 0..2147483647`) on their own,
/// so the relation is the handler's to refuse; the message names the window the fleet would
/// be left with.
pub fn inverted_worker_count_window(
    min_worker_count: i32,
    max_worker_count: i32,
) -> crate::ops::types::ValidationException {
    field_validation_failed(
        "minWorkerCount",
        &format!(
            "minWorkerCount ({min_worker_count}) must be less than or equal to \
             maxWorkerCount ({max_worker_count})"
        ),
    )
}

/// A persisted worker, as `CreateWorker` registered it under its fleet.
#[derive(Debug, Clone)]
pub struct Worker {
    pub worker_id: String,
    pub fleet_id: String,
    pub farm_id: String,
    pub host_properties: Option<crate::ops::types::HostPropertiesRequest>,
    pub status: String,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
}

impl Worker {
    /// The worker's host properties as a response reports them.
    ///
    /// A request carries only the host's own addresses and name
    /// (`HostPropertiesRequest`), while the response shape adds the host EC2 instance's
    /// ARN and type. This emulator launches no instance for a worker, so those two are
    /// reported by nothing; a worker registered without host properties at all has none
    /// to report and the member is left out rather than sent empty.
    pub fn host_properties(&self) -> Option<crate::ops::types::HostPropertiesResponse> {
        let properties = self.host_properties.as_ref()?;
        Some(crate::ops::types::HostPropertiesResponse {
            ip_addresses: properties.ip_addresses.clone(),
            host_name: properties.host_name.clone(),
            ec2_instance_arn: None,
            ec2_instance_type: None,
        })
    }
}

/// A persisted budget, as `CreateBudget` stored it under its farm.
#[derive(Debug, Clone)]
pub struct Budget {
    pub budget_id: String,
    pub farm_id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub status: String,
    pub usage_tracking_resource: crate::ops::types::UsageTrackingResource,
    pub approximate_dollar_limit: f32,
    pub actions: Vec<crate::ops::types::ResponseBudgetAction>,
    pub schedule: crate::ops::types::BudgetSchedule,
    /// What the budget has spent so far. Nothing in this emulator meters rendering usage,
    /// so a budget that has never had usage reported against it has consumed nothing.
    pub approximate_dollar_usage: f32,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    /// When this budget last stopped its queue's scheduling; `None` until a budget action
    /// has fired, which nothing in this emulator does yet.
    pub queue_stopped_at: Option<SystemTime>,
}

impl Budget {
    /// Whether moving this budget to `status` is the one-way `ACTIVE -> INACTIVE`
    /// transition's reverse, which the service refuses.
    ///
    /// A budget leaves `ACTIVE` once and never returns: `INACTIVE` covers an expired,
    /// cancelled or deleted budget (`smithy_query(query="shape", shape="BudgetSummary")`
    /// documents the status that way), none of which can be undone by an update.
    pub fn reactivation_refused(&self, status: &str) -> bool {
        self.status == BUDGET_STATUS_INACTIVE && status == BUDGET_STATUS_ACTIVE
    }
}

/// A persisted queue, as `CreateQueue` stored it under its farm.
#[derive(Debug, Clone)]
pub struct Queue {
    pub queue_id: String,
    pub farm_id: String,
    pub display_name: String,
    pub description: String,
    pub status: String,
    pub default_budget_action: String,
    pub blocked_reason: Option<String>,
    pub role_arn: Option<String>,
    pub job_attachment_settings: Option<crate::ops::types::JobAttachmentSettings>,
    pub job_run_as_user: Option<crate::ops::types::JobRunAsUser>,
    pub required_file_system_location_names: Option<Vec<String>>,
    pub allowed_storage_profile_ids: Option<Vec<String>>,
    pub scheduling_configuration: Option<crate::ops::types::SchedulingConfiguration>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    pub jobs: HashMap<String, Job>,
    /// Every fleet associated with this queue, keyed by `fleetId`, exactly as the model's
    /// paths nest the association
    /// (`/2023-10-12/farms/{farmId}/queue-fleet-associations/{queueId}/{fleetId}`), so an
    /// association is reachable only through its own queue and a queue holds at most one
    /// of them per fleet.
    pub fleet_associations: HashMap<String, QueueFleetAssociation>,
    /// Every limit associated with this queue, keyed by `limitId`, exactly as the model's
    /// paths nest the association
    /// (`/2023-10-12/farms/{farmId}/queue-limit-associations/{queueId}/{limitId}`), so an
    /// association is reachable only through its own queue and a queue holds at most one
    /// of them per limit.
    pub limit_associations: HashMap<String, QueueLimitAssociation>,
}

impl Queue {
    /// Whether any fleet associated with this queue is in a status that schedules the
    /// queue's work onto it.
    ///
    /// A queue with no such association has no worker that could ever pick up its jobs:
    /// the three statuses that are not [`QUEUE_FLEET_ASSOCIATION_STATUS_ACTIVE`] all stop
    /// the fleet scheduling for the queue.
    pub fn schedules_jobs(&self) -> bool {
        self.fleet_associations
            .values()
            .any(|association| association.status == QUEUE_FLEET_ASSOCIATION_STATUS_ACTIVE)
    }
}

/// A persisted queue-fleet association, as `CreateQueueFleetAssociation` stored it under
/// its queue. The pair of ids is its identity — the model mints no id for it.
#[derive(Debug, Clone)]
pub struct QueueFleetAssociation {
    pub queue_id: String,
    pub fleet_id: String,
    pub status: String,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    /// When the clock-owed `STOP_SCHEDULING_AND_* -> STOPPED` move comes due; `None` while
    /// no drain is underway, and again once it has been materialised. See
    /// [`QUEUE_FLEET_ASSOCIATION_STOPPED_AFTER`].
    pub stopped_due_at: Option<Instant>,
}

impl QueueFleetAssociation {
    /// Whether this association has stopped holding the queue's work on the fleet.
    ///
    /// [`QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED`] is the one status in which neither
    /// scheduling nor a drain of the queue's tasks is still attached to the fleet, so it is
    /// the one status in which the association can be taken away.
    pub fn stopped(&self) -> bool {
        self.status == QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED
    }

    /// Materialise the `STOP_SCHEDULING_AND_* -> STOPPED` move the clock already made due.
    ///
    /// A read calls this before answering: the drain was owed at this instant whether or
    /// not anybody called, so applying it on read is reporting, not mutating. It writes no
    /// timestamp — `updatedAt` stays at the update that asked for the stop, so what a read
    /// reports does not depend on when somebody polled — exactly as the fleet's own
    /// clock-owed transition leaves it ([`Fleet::settle_status`]).
    pub fn settle_status(&mut self) {
        if let Some(due) = self.stopped_due_at {
            if Instant::now() >= due {
                self.status = QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED.to_string();
                self.stopped_due_at = None;
            }
        }
    }

    /// Ask this association for `status`, as `UpdateQueueFleetAssociation` names it.
    ///
    /// `ACTIVE` puts the queue's work back on the fleet at once, so nothing is owed by the
    /// clock for it; either `STOP_SCHEDULING_AND_*` is the association draining, which the
    /// clock finishes [`QUEUE_FLEET_ASSOCIATION_STOPPED_AFTER`] later. A drain is the
    /// service stopping the queue's work on the fleet, so an association that has already
    /// stopped has none left to stop: asking it for either stop status leaves it
    /// [`QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED`], and asking for the status it already
    /// carries changes nothing and does not restart a drain underway.
    pub fn ask_for_status(&mut self, status: &str, by: &str) {
        let stopping = status != QUEUE_FLEET_ASSOCIATION_STATUS_ACTIVE;
        if self.status == status || (stopping && self.stopped()) {
            return;
        }
        self.status = status.to_string();
        self.stopped_due_at =
            stopping.then(|| Instant::now() + QUEUE_FLEET_ASSOCIATION_STOPPED_AFTER);
        self.updated_at = Some(SystemTime::now());
        self.updated_by = Some(by.to_string());
    }
}

/// A persisted queue-limit association, as `CreateQueueLimitAssociation` stored it under
/// its queue. The pair of ids is its identity — the model mints no id for it.
#[derive(Debug, Clone)]
pub struct QueueLimitAssociation {
    pub queue_id: String,
    pub limit_id: String,
    pub status: String,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
}

/// A persisted job, as `CreateJob` stored it under its queue.
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub queue_id: String,
    pub farm_id: String,
    pub name: String,
    pub description: Option<String>,
    pub template: Option<String>,
    pub template_type: Option<String>,
    pub lifecycle_status: String,
    pub lifecycle_status_message: String,
    pub priority: i32,
    pub parameters: Option<HashMap<String, crate::ops::types::JobParameter>>,
    pub attachments: Option<crate::ops::types::Attachments>,
    pub storage_profile_id: Option<String>,
    pub target_task_run_status: String,
    /// The job's own run status: what the service has settled its tasks to, as opposed to
    /// the [`target_task_run_status`](Self::target_task_run_status) the request asked for.
    /// It starts at [`TASK_RUN_STATUS_PENDING`] and every later value is set by a
    /// clock-owed move in [`Job::settle`].
    pub task_run_status: String,
    pub max_failed_tasks_count: i32,
    pub max_retries_per_task: i32,
    pub max_worker_count: Option<i32>,
    pub source_job_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub created_by: String,
    pub updated_at: Option<SystemTime>,
    pub updated_by: Option<String>,
    pub started_at: Option<SystemTime>,
    pub ended_at: Option<SystemTime>,
    /// When the clock-owed `CREATE_IN_PROGRESS -> CREATE_COMPLETE` move comes due;
    /// `None` once it has been materialised. See [`JOB_CREATE_COMPLETE_AFTER`].
    pub create_complete_due_at: Option<Instant>,
    /// When the clock-owed reconciliation of this job against the fleets its queue
    /// schedules onto comes due; `None` once it has been materialised. See
    /// [`JOB_RECONCILE_AFTER`].
    pub reconcile_due_at: Option<Instant>,
}

impl Job {
    /// Materialise the status moves the clock already made due.
    ///
    /// A read calls this before answering: each move was owed at this instant whether or
    /// not anybody called, so applying it on read is reporting, not mutating.
    ///
    /// `queue_schedules_jobs` is [`Queue::schedules_jobs`] for the queue holding this job:
    /// the reconciliation places the job against the fleets its queue schedules onto, and
    /// a job no fleet can ever pick up is the one that settles to
    /// [`TASK_RUN_STATUS_NOT_COMPATIBLE`].
    pub fn settle(&mut self, queue_schedules_jobs: bool) {
        let now = Instant::now();
        if let Some(due) = self.create_complete_due_at {
            if now >= due {
                self.lifecycle_status = JOB_LIFECYCLE_CREATE_COMPLETE.to_string();
                self.lifecycle_status_message =
                    "The job was created and is ready to be scheduled.".to_string();
                // Creation has converged on the status the request asked the job to start
                // in, so that is what the job's tasks now carry.
                self.task_run_status = self.target_task_run_status.clone();
                self.create_complete_due_at = None;
            }
        }
        if let Some(due) = self.reconcile_due_at {
            if now >= due {
                if !queue_schedules_jobs && self.awaiting_scheduling() {
                    self.lifecycle_status = JOB_LIFECYCLE_UPDATE_SUCCEEDED.to_string();
                    self.lifecycle_status_message =
                        "The job can't be scheduled because no fleet associated with its \
                         queue can run it."
                            .to_string();
                    self.task_run_status = TASK_RUN_STATUS_NOT_COMPATIBLE.to_string();
                    // The service, not a caller, moved the job, so the update it made is
                    // dated here and `updatedBy` stays absent: no principal made it.
                    self.updated_at = Some(SystemTime::now());
                }
                self.reconcile_due_at = None;
            }
        }
    }

    /// Whether the service still has to find a worker for this job's tasks.
    ///
    /// Only a job whose tasks are [`TASK_RUN_STATUS_READY`] is waiting to be scheduled: a
    /// job started suspended is held back until it is requeued
    /// (`smithy_query(query="doc", shape="CreateJobRequest", member="targetTaskRunStatus")`
    /// documents it as not running until then), and a job whose tasks have reached an end
    /// state is not waiting for anything either.
    fn awaiting_scheduling(&self) -> bool {
        self.task_run_status == TASK_RUN_STATUS_READY
    }

    /// The `targetTaskRunStatus` a read of this job reports.
    ///
    /// The target is the run status the service is still working the job towards, so it is
    /// reported only while the lifecycle says that work is in progress — the statuses the
    /// model spells with an `_IN_PROGRESS` suffix
    /// (`smithy_query(query="shape", shape="JobLifecycleStatus")`). Once the lifecycle has
    /// settled, [`task_run_status`](Self::task_run_status) is what the job reports and the
    /// target is dropped.
    pub fn reported_target_task_run_status(&self) -> Option<String> {
        self.lifecycle_status
            .ends_with("_IN_PROGRESS")
            .then(|| self.target_task_run_status.clone())
    }
}

/// Per-service state for com-amazonaws-deadline-deadline. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDeadlineDeadlineState {
    /// Every farm this emulator holds, keyed by `farmId`. Queues hang off their farm and
    /// jobs off their queue, so there is exactly one copy of each resource.
    pub farms: HashMap<String, Farm>,
    /// `X-Amz-Client-Token` -> the id the first call with that token created, so a retry
    /// of the same create answers with the same resource instead of making a second one.
    /// Reach it only through [`remembered_resource`](Self::remembered_resource) /
    /// [`remember_resource`](Self::remember_resource) / [`forget_resources`](Self::forget_resources),
    /// which are what keep it BOUNDED: deleting a resource forgets its tokens, and the map
    /// never holds more than [`MAX_REMEMBERED_CLIENT_TOKENS`] entries.
    idempotency: HashMap<String, RememberedCreate>,
    /// Bumped once per minted id so two ids minted in the same nanosecond still differ.
    id_sequence: u64,
    /// How many bytes of job templates the stored jobs hold, kept in step with every
    /// job inserted and every job deleted, so the byte budget is released with its jobs.
    stored_template_bytes: usize,
}

/// What one client token created, and when (in `id_sequence` order) it was recorded, so
/// the oldest entry is the one evicted at the cap.
#[derive(Debug, Clone)]
struct RememberedCreate {
    resource_id: String,
    recorded: u64,
}

impl ComAmazonawsDeadlineDeadlineState {
    /// Mint a fresh resource id: `<prefix>-` followed by 32 lowercase hex characters,
    /// which is what every `^farm-[0-9a-f]{32}$` / `^queue-…$` / `^job-…$` label pattern
    /// in this model accepts.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.id_sequence += 1;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(prefix.as_bytes());
        hasher.update(self.id_sequence.to_be_bytes());
        hasher.update(nanos.to_be_bytes());
        let digest = hasher.finalize();
        format!("{prefix}-{}", hex::encode(&digest[..16]))
    }

    /// The resource the earlier create with this client-token key made, if the store still
    /// remembers it.
    pub fn remembered_resource(&self, key: &str) -> Option<&str> {
        self.idempotency
            .get(key)
            .map(|remembered| remembered.resource_id.as_str())
    }

    /// Remember what a create with this client-token key made.
    ///
    /// Bounded on purpose: a caller that loops creates with a fresh token each time must
    /// not be able to grow this map without limit, so at the cap the OLDEST entry is
    /// evicted. Eviction only costs that caller the retry-deduplication of a create it
    /// made thousands of creates ago; it never affects a stored resource.
    pub fn remember_resource(&mut self, key: String, resource_id: &str) {
        self.id_sequence += 1;
        let recorded = self.id_sequence;
        self.idempotency.insert(
            key,
            RememberedCreate {
                resource_id: resource_id.to_string(),
                recorded,
            },
        );
        while self.idempotency.len() > MAX_REMEMBERED_CLIENT_TOKENS {
            let Some(oldest) = self
                .idempotency
                .iter()
                .min_by_key(|(_, remembered)| remembered.recorded)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.idempotency.remove(&oldest);
        }
    }

    /// Forget the client tokens of resources that no longer exist: a delete releases the
    /// token entries of everything it removed, so the map does not outlive its resources.
    pub fn forget_resources(&mut self, removed: &[String]) {
        let removed: ::std::collections::HashSet<&str> =
            removed.iter().map(String::as_str).collect();
        self.idempotency
            .retain(|_, remembered| !removed.contains(remembered.resource_id.as_str()));
    }

    /// Would storing `bytes` more of job template fit inside
    /// [`MAX_STORED_TEMPLATE_BYTES`]?
    pub fn template_bytes_fit(&self, bytes: usize) -> bool {
        self.stored_template_bytes.saturating_add(bytes) <= MAX_STORED_TEMPLATE_BYTES
    }

    /// Charge `bytes` of job template to the budget, as a job holding them is stored.
    pub fn charge_template_bytes(&mut self, bytes: usize) {
        self.stored_template_bytes = self.stored_template_bytes.saturating_add(bytes);
    }

    /// Release the template bytes of jobs that have been deleted.
    pub fn release_template_bytes(&mut self, bytes: usize) {
        self.stored_template_bytes = self.stored_template_bytes.saturating_sub(bytes);
    }

    /// The farm `farm_id` names, or `None` when this emulator holds no such farm.
    pub fn farm(&self, farm_id: &str) -> Option<&Farm> {
        self.farms.get(farm_id)
    }

    /// The queue `queue_id` names UNDER `farm_id`: a queue of another farm is not found
    /// here, because the model reaches a queue only through its own farm's path.
    pub fn queue(&self, farm_id: &str, queue_id: &str) -> Option<&Queue> {
        self.farms.get(farm_id)?.queues.get(queue_id)
    }

    /// The fleet `fleet_id` names UNDER `farm_id`, exactly as stored.
    pub fn fleet(&self, farm_id: &str, fleet_id: &str) -> Option<&Fleet> {
        self.farms.get(farm_id)?.fleets.get(fleet_id)
    }

    /// Mutable form of [`fleet`](Self::fleet).
    pub fn fleet_mut(&mut self, farm_id: &str, fleet_id: &str) -> Option<&mut Fleet> {
        self.farms.get_mut(farm_id)?.fleets.get_mut(fleet_id)
    }

    /// The worker `worker_id` names UNDER `farm_id`/`fleet_id`: a worker of another fleet
    /// is not found here, because the model reaches a worker only through its own fleet's
    /// path.
    pub fn worker(&self, farm_id: &str, fleet_id: &str, worker_id: &str) -> Option<&Worker> {
        self.fleet(farm_id, fleet_id)?.workers.get(worker_id)
    }

    /// Mutable form of [`queue`](Self::queue).
    pub fn queue_mut(&mut self, farm_id: &str, queue_id: &str) -> Option<&mut Queue> {
        self.farms.get_mut(farm_id)?.queues.get_mut(queue_id)
    }

    /// The fleet `fleet_id` names UNDER `farm_id`, with any clock-owed status transition
    /// already materialised: a fleet of another farm is not found here, because the model
    /// reaches a fleet only through its own farm's path.
    pub fn settled_fleet(&mut self, farm_id: &str, fleet_id: &str) -> Option<&Fleet> {
        let fleet = self.farms.get_mut(farm_id)?.fleets.get_mut(fleet_id)?;
        fleet.settle_status();
        Some(fleet)
    }

    /// Every fleet of `farm_id`, each with any clock-owed status transition already
    /// materialised, oldest first with the id breaking a tie so paging over them is
    /// stable. `None` when this emulator holds no such farm.
    pub fn settled_fleets(&mut self, farm_id: &str) -> Option<Vec<Fleet>> {
        let farm = self.farms.get_mut(farm_id)?;
        let mut fleets: Vec<Fleet> = farm
            .fleets
            .values_mut()
            .map(|fleet| {
                fleet.settle_status();
                fleet.clone()
            })
            .collect();
        fleets.sort_by(|left, right| {
            (left.created_at, &left.fleet_id).cmp(&(right.created_at, &right.fleet_id))
        });
        Some(fleets)
    }

    /// The budget `budget_id` names UNDER `farm_id`: a budget of another farm is not
    /// found here, because the model reaches a budget only through its own farm's path.
    pub fn budget(&self, farm_id: &str, budget_id: &str) -> Option<&Budget> {
        self.farms.get(farm_id)?.budgets.get(budget_id)
    }

    /// Mutable form of [`budget`](Self::budget).
    pub fn budget_mut(&mut self, farm_id: &str, budget_id: &str) -> Option<&mut Budget> {
        self.farms.get_mut(farm_id)?.budgets.get_mut(budget_id)
    }

    /// The storage profile `storage_profile_id` names UNDER `farm_id`: a profile of
    /// another farm is not found here, because the model reaches a storage profile only
    /// through its own farm's path.
    pub fn storage_profile(
        &self,
        farm_id: &str,
        storage_profile_id: &str,
    ) -> Option<&StorageProfile> {
        self.farms
            .get(farm_id)?
            .storage_profiles
            .get(storage_profile_id)
    }

    /// The limit `limit_id` names UNDER `farm_id`: a limit of another farm is not found
    /// here, because the model reaches a limit only through its own farm's path.
    pub fn limit(&self, farm_id: &str, limit_id: &str) -> Option<&Limit> {
        self.farms.get(farm_id)?.limits.get(limit_id)
    }

    /// The job `job_id` names under `farm_id`/`queue_id`, with any clock-owed status move
    /// already materialised.
    pub fn settled_job(&mut self, farm_id: &str, queue_id: &str, job_id: &str) -> Option<&Job> {
        let queue = self.farms.get_mut(farm_id)?.queues.get_mut(queue_id)?;
        let schedules_jobs = queue.schedules_jobs();
        let job = queue.jobs.get_mut(job_id)?;
        job.settle(schedules_jobs);
        Some(job)
    }

    /// Materialise every job of `queue_id`'s owed status moves against the queue's fleet
    /// associations as they stand NOW.
    ///
    /// A call that changes those associations runs this first, so each job is reconciled
    /// against what its queue could schedule onto while the move was owed rather than
    /// against what the call is about to make true.
    pub fn settle_jobs(&mut self, farm_id: &str, queue_id: &str) {
        let Some(queue) = self.queue_mut(farm_id, queue_id) else {
            return;
        };
        let schedules_jobs = queue.schedules_jobs();
        for job in queue.jobs.values_mut() {
            job.settle(schedules_jobs);
        }
    }

    /// The association between `queue_id` and `fleet_id` UNDER `farm_id`: an association
    /// of another farm's queue is not found here, because the model reaches an association
    /// only through its own farm's path.
    pub fn queue_fleet_association(
        &self,
        farm_id: &str,
        queue_id: &str,
        fleet_id: &str,
    ) -> Option<&QueueFleetAssociation> {
        self.queue(farm_id, queue_id)?
            .fleet_associations
            .get(fleet_id)
    }

    /// Mutable form of [`queue_fleet_association`](Self::queue_fleet_association).
    pub fn queue_fleet_association_mut(
        &mut self,
        farm_id: &str,
        queue_id: &str,
        fleet_id: &str,
    ) -> Option<&mut QueueFleetAssociation> {
        self.queue_mut(farm_id, queue_id)?
            .fleet_associations
            .get_mut(fleet_id)
    }

    /// [`queue_fleet_association`](Self::queue_fleet_association) with any clock-owed drain
    /// already materialised: what a read of the association reports.
    pub fn settled_queue_fleet_association(
        &mut self,
        farm_id: &str,
        queue_id: &str,
        fleet_id: &str,
    ) -> Option<&QueueFleetAssociation> {
        let association = self.queue_fleet_association_mut(farm_id, queue_id, fleet_id)?;
        association.settle_status();
        Some(association)
    }

    /// The association between `queue_id` and `limit_id` UNDER `farm_id`: an association
    /// of another farm's queue is not found here, because the model reaches an association
    /// only through its own farm's path.
    pub fn queue_limit_association(
        &self,
        farm_id: &str,
        queue_id: &str,
        limit_id: &str,
    ) -> Option<&QueueLimitAssociation> {
        self.queue(farm_id, queue_id)?
            .limit_associations
            .get(limit_id)
    }

    /// The limit already associated with `queue_id` that declares
    /// `amount_requirement_name`, lowest id first so the answer does not depend on map
    /// order. `None` when none does.
    ///
    /// A limit's `amountRequirementName` is what a step of a job template names to declare
    /// the limit requirement, so two limits declaring the same one would leave a step's
    /// requirement ambiguous on this queue.
    pub fn limit_associated_with_queue_requiring(
        &self,
        farm_id: &str,
        queue_id: &str,
        amount_requirement_name: &str,
    ) -> Option<&str> {
        let farm = self.farms.get(farm_id)?;
        farm.queues
            .get(queue_id)?
            .limit_associations
            .keys()
            .filter(|limit_id| {
                farm.limits.get(*limit_id).is_some_and(|limit| {
                    limit.amount_requirement_name == amount_requirement_name
                })
            })
            .map(String::as_str)
            .min()
    }

    /// The queue of `farm_id` that still has `fleet_id` associated with it, lowest id
    /// first so the answer does not depend on map order. `None` when no queue does.
    pub fn queue_associated_with_fleet(&self, farm_id: &str, fleet_id: &str) -> Option<&str> {
        let farm = self.farms.get(farm_id)?;
        farm.queues
            .values()
            .filter(|queue| queue.fleet_associations.contains_key(fleet_id))
            .map(|queue| queue.queue_id.as_str())
            .min()
    }

    /// The queue of `farm_id` that still lists `storage_profile_id` among the storage
    /// profiles it allows, lowest id first so the answer does not depend on map order.
    /// `None` when no queue does.
    pub fn queue_allowing_storage_profile(
        &self,
        farm_id: &str,
        storage_profile_id: &str,
    ) -> Option<&str> {
        let farm = self.farms.get(farm_id)?;
        farm.queues
            .values()
            .filter(|queue| {
                queue
                    .allowed_storage_profile_ids
                    .as_deref()
                    .is_some_and(|allowed| {
                        allowed.iter().any(|allowed| allowed == storage_profile_id)
                    })
            })
            .map(|queue| queue.queue_id.as_str())
            .min()
    }

    /// The queue of `farm_id` that still has `limit_id` associated with it, lowest id
    /// first so the answer does not depend on map order. `None` when no queue does.
    pub fn queue_associated_with_limit(&self, farm_id: &str, limit_id: &str) -> Option<&str> {
        let farm = self.farms.get(farm_id)?;
        farm.queues
            .values()
            .filter(|queue| queue.limit_associations.contains_key(limit_id))
            .map(|queue| queue.queue_id.as_str())
            .min()
    }
}

/// Scope a client token to the operation and the parent resource it was used on, so the
/// same token replayed on two different creates cannot collide.
pub fn idempotency_key(operation: &str, parent: &str, client_token: &str) -> String {
    format!("{operation}/{parent}/{client_token}")
}

/// The principal this request is made by, derived from the caller's own SigV4 access key
/// id. Stored once at create time, so every later read of the resource reports the same
/// `createdBy`.
pub fn caller_principal(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    let akid = ::engine::extract_akid(&ctx.parts().headers);
    ::engine::emulator_arn("iam", "", &format!("user/{akid}"))
}

/// A stored instant as the modeled `date-time` timestamp the response carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The 404 this service answers for a resource it does not hold.
///
/// One sentence serves every resource type this service refuses, with the type and the id
/// the request named filled in; the trailing period is part of it. This is how the service
/// itself words a missing resource, as the responses recorded from it show for each of the
/// types below.
pub fn not_found(
    resource_type: &str,
    resource_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    crate::ops::types::ResourceNotFoundException {
        message: format!("Resource of type {resource_type} with id {resource_id} does not exist."),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
        context: None,
    }
}

/// The id this service reports for a queue-fleet association.
///
/// The association carries none of its own — the model mints no id for it and reaches it by
/// the three ids of its path — so it is named by all three, joined the way this service
/// names a composite resource: the first two separated by a comma and the last by `and`.
pub fn queue_fleet_association_id(farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    format!("{farm_id}, {queue_id} and {fleet_id}")
}

/// The 404 this service answers for a queue-fleet association it does not hold.
///
/// The resource reported missing is the association, named by
/// [`queue_fleet_association_id`], and its type is spelled the way the API path spells it.
pub fn queue_fleet_association_not_found(
    farm_id: &str,
    queue_id: &str,
    fleet_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    not_found(
        RESOURCE_TYPE_QUEUE_FLEET_ASSOCIATION,
        &queue_fleet_association_id(farm_id, queue_id, fleet_id),
    )
}

/// The id this service reports for a queue-limit association.
///
/// The association carries none of its own — the model mints no id for it and reaches it by
/// the three ids of its path — so it is named the same way
/// [`queue_fleet_association_id`] names the other association of this service.
pub fn queue_limit_association_id(farm_id: &str, queue_id: &str, limit_id: &str) -> String {
    format!("{farm_id}, {queue_id} and {limit_id}")
}

/// The 404 this service answers for a queue-limit association it does not hold.
///
/// The resource reported missing is the association, named by
/// [`queue_limit_association_id`], and its type is spelled the way the API path spells it.
pub fn queue_limit_association_not_found(
    farm_id: &str,
    queue_id: &str,
    limit_id: &str,
) -> crate::ops::types::ResourceNotFoundException {
    not_found(
        RESOURCE_TYPE_QUEUE_LIMIT_ASSOCIATION,
        &queue_limit_association_id(farm_id, queue_id, limit_id),
    )
}

/// The 402 this service answers for a create that would pass one of this emulator's
/// stored-resource bounds. `resource_type` is the level that is full.
pub fn quota_exceeded(
    resource_type: &str,
    message: &str,
) -> crate::ops::types::ServiceQuotaExceededException {
    crate::ops::types::ServiceQuotaExceededException {
        message: message.to_string(),
        // From `smithy_query(query="shape", shape="ServiceQuotaExceededExceptionReason")`:
        // SERVICE_QUOTA_EXCEEDED_EXCEPTION, KMS_KEY_LIMIT_EXCEEDED,
        // DEPENDENCY_LIMIT_EXCEEDED.
        reason: "SERVICE_QUOTA_EXCEEDED_EXCEPTION".to_string(),
        resource_type: resource_type.to_string(),
        // The service that owns the quota, as the model's `awsService.arnNamespace` names
        // it (`smithy_query(query="service_info")`).
        service_code: "deadline".to_string(),
        quota_code: QUOTA_CODE_RESOURCE_COUNT.to_string(),
        resource_id: None,
        context: None,
    }
}

/// The 409 this service answers when a fleet already holds every worker it can hold.
///
/// The service documents a fleet's worker count as limited to its `maxWorkerCount`, so the
/// fleet is the resource in conflict and the message names the capacity the registration
/// ran into; `capacity` is read from the fleet, never from the request.
pub fn fleet_at_worker_capacity(
    fleet_id: &str,
    capacity: i64,
) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!(
            "The fleet {fleet_id} already has its maximum worker count of {capacity} workers."
        ),
        // From `smithy_query(query="shape", shape="ConflictExceptionReason")`:
        // CONFLICT_EXCEPTION, CONCURRENT_MODIFICATION, RESOURCE_ALREADY_EXISTS,
        // RESOURCE_IN_USE, STATUS_CONFLICT. Every worker the fleet holds is in use by it,
        // which is what leaves no room for another.
        reason: "RESOURCE_IN_USE".to_string(),
        resource_id: fleet_id.to_string(),
        resource_type: RESOURCE_TYPE_FLEET.to_string(),
        context: None,
    }
}

/// The 409 this service answers when a fleet a queue is still associated with is deleted.
///
/// The association is what the deletion would leave pointing at a fleet that no longer
/// exists, so the fleet is the resource in conflict and the message names the queue that
/// holds it; `queue_id` is read from the store, never from the request.
pub fn fleet_associated_with_queue(
    fleet_id: &str,
    queue_id: &str,
) -> crate::ops::types::ConflictException {
    crate::ops::types::ConflictException {
        message: format!(
            "The fleet {fleet_id} can't be deleted because it is associated with the \
             queue {queue_id}."
        ),
        // From `smithy_query(query="shape", shape="ConflictExceptionReason")`:
        // CONFLICT_EXCEPTION, CONCURRENT_MODIFICATION, RESOURCE_ALREADY_EXISTS,
        // RESOURCE_IN_USE, STATUS_CONFLICT. The queue's association is a use of the fleet.
        reason: "RESOURCE_IN_USE".to_string(),
        resource_id: fleet_id.to_string(),
        resource_type: RESOURCE_TYPE_FLEET.to_string(),
        context: None,
    }
}

/// The 400 this service answers when a storage profile a queue still allows is deleted.
///
/// Same rule as [`fleet_associated_with_queue`] — a resource a queue's configuration still
/// points at is not deleted out from under it — answered through the only client-input
/// error `DeleteStorageProfile` models: it declares neither `ConflictException` nor
/// `ResourceNotFoundException`, so its refusals go out as `ValidationException`
/// (`smithy_query(query="operation", operation="DeleteStorageProfile")`). The message names
/// the queue that holds the reference; `queue_id` is read from the store, never from the
/// request.
pub fn storage_profile_allowed_by_queue(
    storage_profile_id: &str,
    queue_id: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: format!(
            "The storage profile {storage_profile_id} can't be deleted because it is \
             associated with the queue {queue_id}."
        ),
        // From `smithy_query(query="shape", shape="ValidationExceptionReason")`:
        // UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER. Every member of
        // the request satisfies its own constraints — what is refused is the state the
        // deletion would leave behind — so no one field is at fault and the refusal
        // carries no field list.
        reason: "OTHER".to_string(),
        field_list: None,
        context: None,
    }
}

/// The 400 this service answers when a limit a queue is still associated with is deleted.
///
/// `smithy_query(query="doc", shape="DeleteLimit")` states the precondition: "Before you
/// delete a limit you must use the DeleteQueueLimitAssociation operation to remove the
/// association with any queues." The association is a live reference to the limit — it is
/// what `GetQueueLimitAssociation` resolves — so the limit is not deleted out from under
/// the queue that holds it. Same rule as [`storage_profile_allowed_by_queue`], and
/// answered through the only client-input error `DeleteLimit` models: it declares neither
/// `ConflictException` nor `ResourceNotFoundException`, so its refusals go out as
/// `ValidationException` (`smithy_query(query="operation", operation="DeleteLimit")`). The
/// message names the queue that holds the association; `queue_id` is read from the store,
/// never from the request.
pub fn limit_associated_with_queue(
    limit_id: &str,
    queue_id: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: format!(
            "The limit {limit_id} can't be deleted because it is associated with the \
             queue {queue_id}."
        ),
        // From `smithy_query(query="shape", shape="ValidationExceptionReason")`:
        // UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER. Every member of
        // the request satisfies its own constraints — what is refused is the state the
        // deletion would leave behind — so no one field is at fault and the refusal
        // carries no field list.
        reason: "OTHER".to_string(),
        field_list: None,
        context: None,
    }
}

/// The 409 this service answers when a queue-fleet association is deleted before it has
/// stopped.
///
/// An association that is not [`QUEUE_FLEET_ASSOCIATION_STATUS_STOPPED`] is still holding
/// the queue's work on the fleet — scheduling it (`ACTIVE`) or draining it (the two
/// `STOP_SCHEDULING_AND_*` statuses) — so the association itself is the resource in
/// conflict and the status it is in is the one value the message varies on; it is read from
/// the store, never from the request.
pub fn queue_fleet_association_not_stopped(
    farm_id: &str,
    queue_id: &str,
    fleet_id: &str,
    status: &str,
) -> crate::ops::types::ConflictException {
    let resource_id = queue_fleet_association_id(farm_id, queue_id, fleet_id);
    crate::ops::types::ConflictException {
        message: format!("QueueFleetAssociation is in {status} status and cannot be deleted."),
        // From `smithy_query(query="shape", shape="ConflictExceptionReason")`:
        // CONFLICT_EXCEPTION, CONCURRENT_MODIFICATION, RESOURCE_ALREADY_EXISTS,
        // RESOURCE_IN_USE, STATUS_CONFLICT. What conflicts is the status the association
        // is in, not another caller's hold on it.
        reason: "STATUS_CONFLICT".to_string(),
        resource_id,
        resource_type: RESOURCE_TYPE_QUEUE_FLEET_ASSOCIATION.to_string(),
        context: None,
    }
}

/// The 500 a handler answers when the engine it was built with holds no store for this
/// service. A handler must never build its own engine instead — that store is private and
/// is dropped when the request returns.
pub fn state_unavailable() -> crate::ops::types::InternalServerErrorException {
    crate::ops::types::InternalServerErrorException {
        message: "Deadline Cloud can't process your request right now. Try again later."
            .to_string(),
        retry_after_seconds: None,
    }
}

/// The modeled 400 for a request whose values do not hold together, naming the member at
/// fault the way the boundary validator does.
pub fn field_validation_failed(
    member: &str,
    message: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: message.to_string(),
        // From `smithy_query(query="shape", shape="ValidationExceptionReason")`:
        // UNKNOWN_OPERATION, CANNOT_PARSE, FIELD_VALIDATION_FAILED, OTHER.
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(vec![crate::ops::types::ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
        context: None,
    }
}

/// The modeled 400 for a payload the service cannot parse as the type the request said it
/// was.
pub fn cannot_parse(member: &str, message: &str) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: message.to_string(),
        reason: "CANNOT_PARSE".to_string(),
        field_list: Some(vec![crate::ops::types::ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
        context: None,
    }
}

/// The refusal this service answers when an update asks a budget to go back to
/// [`BUDGET_STATUS_ACTIVE`].
///
/// Entirely fixed text: the budget's id, name and current status are interpolated
/// nowhere in it.
pub const REACTIVATION_REFUSED: &str = "Reactivating of INACTIVE budget is not allowed";

/// Modeled `@length` maximum of a budget-action list.
///
/// `BudgetActionsToAdd` and `BudgetActionsToRemove` both carry `length {min 0, max 10}`
/// (`smithy_query(query="operation", operation="CreateBudget,UpdateBudget")`). There is
/// no too-short finding to make for either: the modeled minimum is 0, and no list holds
/// fewer entries than that.
pub const MAX_BUDGET_ACTIONS: usize = 10;

/// Modeled `@length` maximum of the action list a budget HOLDS.
///
/// `ResponseBudgetActionList` — the target of `GetBudget.actions` — carries
/// `length {min 0, max 10}` (`smithy_query(query="operation", operation="GetBudget")`),
/// so the model bounds the STORED list as well as each request's list. That is what keeps
/// repeated updates from growing one budget's actions without limit, and the response that
/// reports them from growing with it: an update that would leave the budget holding more
/// than this is refused and applies nothing. Numerically the same figure as
/// [`MAX_BUDGET_ACTIONS`], from a different shape, so each keeps its own citation.
pub const MAX_STORED_BUDGET_ACTIONS: usize = 10;

/// Modeled `@range` minimum of `ConsumedUsageLimit`, the target of
/// `approximateDollarLimit`: `range {min 0.01}`, with no maximum.
pub const MIN_APPROXIMATE_DOLLAR_LIMIT: f32 = 0.01;

/// Modeled `@range` minimum of `ThresholdPercentage`, the target of a budget action's
/// `thresholdPercentage`: `range {min 0, max 100}`.
pub const MIN_THRESHOLD_PERCENTAGE: f32 = 0.0;

/// Modeled `@range` maximum of `ThresholdPercentage`. See [`MIN_THRESHOLD_PERCENTAGE`].
pub const MAX_THRESHOLD_PERCENTAGE: f32 = 100.0;

/// The clause the budget-body validator appends to every finding list it reports.
///
/// A budget request body declares `float` members — `approximateDollarLimit` and each
/// budget action's `thresholdPercentage` (`smithy_query(query="operation",
/// operation="CreateBudget")` reports `"type": "float"` on both) — and the validator that
/// checks those bodies does not implement the `float` format attribute, so it states that
/// alongside whatever else it found. Fixed text: nothing of the request reaches it.
const FLOAT_FORMAT_UNSUPPORTED: &str = "format attribute \"float\" not supported";

/// Ceiling on the findings one refused body reports.
///
/// Every finding interpolates both the violated bound and the value sent, and the number
/// of findings grows with the number of budget actions the request carries — which the
/// element-count bound no longer caps before the handler sees it. Capping the COUNT is
/// what keeps a refusal from amplifying an oversized body into a much larger message; a
/// caller cannot act on more clauses than this anyway.
const MAX_BUDGET_FINDINGS: usize = 100;

/// One finding of the validator the budget plane checks its request bodies with: the
/// member at fault, and the sentence the validator states about it.
///
/// Held as data rather than as a finished message because one refused body reports EVERY
/// member it found at fault — as one bracketed list in the message, and member by member
/// in the exception's `fieldList`.
#[derive(Debug, Clone)]
pub struct BudgetFinding {
    member: String,
    detail: String,
}

impl BudgetFinding {
    /// A list member holding more entries than its `@length` maximum allows.
    pub fn array_too_long(member: String, max: usize, found: usize) -> Self {
        Self {
            detail: format!(
                "array is too long: must have at most {max} elements but \
                 instance has {found} elements"
            ),
            member,
        }
    }

    /// A numeric member below its `@range` minimum.
    pub fn below_minimum(member: String, min: f32, found: f32) -> Self {
        Self {
            detail: format!(
                "numeric instance is lower than the required minimum \
                 (minimum: {min}, found: {found})"
            ),
            member,
        }
    }

    /// A numeric member above its `@range` maximum.
    pub fn above_maximum(member: String, max: f32, found: f32) -> Self {
        Self {
            detail: format!(
                "numeric instance is greater than the required maximum \
                 (maximum: {max}, found: {found})"
            ),
            member,
        }
    }
}

/// The dotted path of one budget action's member, spelled the way this framework's
/// boundary validator spells a path that steps through a list element
/// (`actions.1.member.thresholdPercentage`). `index` is 0-based; the path is 1-based.
pub fn action_member_path(list_member: &str, index: usize, member: &str) -> String {
    use ::aws_smithy_schema_server::constraints::path;
    path::join(&[
        list_member.to_string(),
        path::element(index + 1),
        path::member(member),
    ])
}

/// Every finding a budget-action list carried under `member` contributes: its own
/// element-count bound, and each action's `thresholdPercentage` range.
///
/// `thresholds` is the list's actions in request order, as the `thresholdPercentage`
/// each one sets.
pub fn check_budget_actions(member: &str, thresholds: &[f32], findings: &mut Vec<BudgetFinding>) {
    if thresholds.len() > MAX_BUDGET_ACTIONS {
        findings.push(BudgetFinding::array_too_long(
            member.to_string(),
            MAX_BUDGET_ACTIONS,
            thresholds.len(),
        ));
    }
    for (index, threshold) in thresholds.iter().copied().enumerate() {
        if findings.len() >= MAX_BUDGET_FINDINGS {
            return;
        }
        let path = action_member_path(member, index, "thresholdPercentage");
        if threshold < MIN_THRESHOLD_PERCENTAGE {
            findings.push(BudgetFinding::below_minimum(
                path,
                MIN_THRESHOLD_PERCENTAGE,
                threshold,
            ));
        } else if threshold > MAX_THRESHOLD_PERCENTAGE {
            findings.push(BudgetFinding::above_maximum(
                path,
                MAX_THRESHOLD_PERCENTAGE,
                threshold,
            ));
        }
    }
}

/// The finding `approximateDollarLimit` contributes, if any.
///
/// Compared at the precision the member carries on the wire (`float`), so the modeled
/// minimum itself is accepted: widening a `float` to a wider type first puts the value
/// just below the bound and refuses a request that satisfies it.
pub fn check_approximate_dollar_limit(
    member: &str,
    value: f32,
    findings: &mut Vec<BudgetFinding>,
) {
    if value < MIN_APPROXIMATE_DOLLAR_LIMIT {
        findings.push(BudgetFinding::below_minimum(
            member.to_string(),
            MIN_APPROXIMATE_DOLLAR_LIMIT,
            value,
        ));
    }
}

/// The modeled 400 a budget request body is refused with once the validator has found
/// anything at fault: every finding as one bracketed list followed by the fixed clause
/// the validator always appends, and every member at fault named in `fieldList`.
///
/// Answer it INSTEAD of storing anything: a refused body leaves no budget behind.
pub fn budget_body_invalid(findings: &[BudgetFinding]) -> crate::ops::types::ValidationException {
    let mut stated: Vec<&str> = findings.iter().map(|f| f.detail.as_str()).collect();
    stated.push(FLOAT_FORMAT_UNSUPPORTED);
    crate::ops::types::ValidationException {
        message: format!("[{}]", stated.join(", ")),
        reason: "FIELD_VALIDATION_FAILED".to_string(),
        field_list: Some(
            findings
                .iter()
                .map(|f| crate::ops::types::ValidationExceptionField {
                    name: f.member.clone(),
                    message: f.detail.clone(),
                })
                .collect(),
        ),
        context: None,
    }
}

/// One page of a sorted key list, plus the token the next page starts at.
///
/// `next_token` is the key the page starts at (the token a previous page handed back);
/// an unknown token is a client error, so this answers `Err(message)` for it.
pub fn page_keys(
    sorted: &[String],
    next_token: Option<&str>,
    max_results: i32,
) -> Result<(Vec<String>, Option<String>), String> {
    let start = match next_token {
        None => 0,
        Some(token) => sorted
            .iter()
            .position(|key| key == token)
            .ok_or_else(|| format!("The nextToken {token} isn't valid."))?,
    };
    let page_size = max_results.clamp(1, MAX_PAGE_SIZE) as usize;
    let page: Vec<String> = sorted.iter().skip(start).take(page_size).cloned().collect();
    let next = sorted.get(start + page_size).cloned();
    Ok((page, next))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDeadlineDeadlineState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDeadlineDeadlineState>)
}
