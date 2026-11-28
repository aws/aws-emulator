// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as a JOB or a RECOVERY POINT moves: one
//! event per state change of a backup job, one per state change of a copy job, one per
//! state change of a restore job and one per state change of a recovery point.
//!
//! # The rule this module implements
//!
//! AWS Backup does not only record a job's progress for a reader to poll: every state a
//! job MOVES INTO after the call that started it is PUBLISHED onto the account's default
//! EventBridge bus, under the `source` this service owns ([`EVENT_SOURCE`]) and the
//! `detail-type` of the KIND of thing that moved ([`BACKUP_JOB_STATE_CHANGE`],
//! [`COPY_JOB_STATE_CHANGE`], [`RESTORE_JOB_STATE_CHANGE`],
//! [`RECOVERY_POINT_STATE_CHANGE`]), carrying that record's own stored members — the state
//! it moved into among them — as the event's `detail` and the recovery point the event is
//! about in `resources`. Each `detail` member is named after the member the MOVED RECORD's
//! own API reports it under, with a lower-case first letter, so the member carrying the
//! state is `state` for a record the model gives a `State` (a backup job, a copy job) and
//! `status` for one it gives a `Status` (a restore job, a recovery point).
//! The same holds for the RECOVERY POINT a backup or a copy writes:
//! it is a resource of its own with a status of its own, so each state it moves into is
//! published too, which is what a client watching for a backup to become available reacts
//! to. A rule on that bus whose `EventPattern` matches then delivers the envelope
//! unchanged to every one of that rule's targets, so a client that reacts to a backup, a
//! copy, a restore or a recovery point through EventBridge observes it without calling AWS
//! Backup again — and ONE rule whose pattern lists several detail-types receives every
//! kind of envelope it names.
//!
//! The state a START call stored is not a move: it is what that call's own response
//! reports, so the snapshot is seeded with it and the events published are the transitions
//! that follow it (the settle to `COMPLETED` the clock owes, and any later one).
//!
//! # How a change becomes an event
//!
//! Nothing here decides WHEN anything moves: the stored job and the clock do
//! ([`super::ComAmazonawsBackupCryocontrollerusermanagerState::settle_job`],
//! [`super::ComAmazonawsBackupCryocontrollerusermanagerState::settle_copy_job`],
//! [`super::ComAmazonawsBackupCryocontrollerusermanagerState::settle_restore_job`],
//! [`super::ComAmazonawsBackupCryocontrollerusermanagerState::settle_recovery_points`]),
//! and a
//! state they write is the fact an event reports. So this module is a DIFF rather than a
//! second state machine:
//! [`super::ComAmazonawsBackupCryocontrollerusermanagerState::record_job_state_changes`]
//! compares every stored job against the state last published for it ([`JobEventLog`],
//! held in the store beside the jobs) and queues one change per state that differs —
//! whichever call materialised it, and without any transition site having to remember to
//! announce itself.
//!
//! A queued change is published by [`publish_job_state_changes`], which runs with NO lock
//! held: EventBridge is reached the way every emulated service reaches another, through the
//! framework's in-process callback and the tree's one client for that hop
//! ([`::service_common::eventbridge_client`]), so this crate names no other service's crate
//! and the publication opens no socket off this process. Each change is published in the
//! Region its own JOB belongs to, read off that job's stored vault ARN, so which call or
//! clock tick drains the queue decides nothing about where an event arrives. Publication is
//! best effort — an emulator with no EventBridge mounted has nowhere to publish — so a
//! failure is logged and the operation's own result stands.
//!
//! The queue is BOUNDED ([`MAX_PENDING_EVENTS`], oldest dropped first) because every
//! transition appends to it, one drain publishes at most [`MAX_PUBLISHED_PER_DRAIN`]
//! changes, and the snapshot is pruned to the jobs the store still holds, so none of the
//! three grows with the number of calls served.

use std::collections::{BTreeMap, VecDeque};
use std::time::SystemTime;

use super::{
    BackupJob, ComAmazonawsBackupCryocontrollerusermanagerState, CopyJobRecord, RecoveryPoint,
    RestoreJobRecord,
};

/// The `source` of every event AWS Backup publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry (`super::ARN_NAMESPACE`).
pub const EVENT_SOURCE: &str = "aws.backup";

/// The `detail-type` of the event published when a BACKUP job changes state.
pub const BACKUP_JOB_STATE_CHANGE: &str = "Backup Job State Change";

/// The `detail-type` of the event published when a COPY job changes state.
pub const COPY_JOB_STATE_CHANGE: &str = "Copy Job State Change";

/// The `detail-type` of the event published when a RESTORE job changes state.
pub const RESTORE_JOB_STATE_CHANGE: &str = "Restore Job State Change";

/// The `detail-type` of the event published when a RECOVERY POINT changes state.
///
/// A recovery point is a resource of its own, not a job: it is what a backup or a copy
/// WROTE, and it moves through the statuses `RecoveryPointStatus` declares
/// (`smithy_query(query="shape", shape="com.amazonaws.backup#RecoveryPointStatus")`:
/// `CREATING`/`COMPLETED`/`AVAILABLE`/`PARTIAL`/`STOPPED`/`DELETING`/`EXPIRED`) on a clock
/// of its own, so its moves are announced under their own detail-type rather than folded
/// into the job's.
pub const RECOVERY_POINT_STATE_CHANGE: &str = "Recovery Point State Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// transition appends to it, so without a bound a client looping on starts could grow it
/// without end. The oldest change is dropped first, so the transitions a live job just
/// made are the ones that survive. It is far above what a realistic burst produces (a job
/// contributes one change per state it takes).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a batch holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE batch of a drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one request dispatching all of
/// it would fan out to a rule's targets that many times: this caps one batch, and
/// [`publish_job_state_changes`] repeats a batch until the queue is empty, so nothing waits
/// for a later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// One state change waiting to be published.
///
/// The `detail` is rendered when the change is RECORDED, so the event reports the record as
/// it was at that instant rather than as a later drain finds it. Every change this service
/// publishes rides this one queue — a job's and a recovery point's moves, and the mutations
/// of a backup plan ([`super::plan_events`]) — so there is ONE drain rather than one per
/// kind of record.
#[derive(Debug, Clone)]
pub struct JobStateChange {
    /// The `detail-type` the event carries.
    pub detail_type: &'static str,
    /// The Region whose default bus this change is published onto: the Region the job
    /// belongs to, read off its own stored vault ARN rather than off the call that
    /// published it, so a job started in one Region is never announced in another.
    pub region: String,
    /// The ARN of the recovery point the job is about — the one a backup or a copy WROTE,
    /// and the one a restore read FROM — which the envelope carries in
    /// `resources`: the event names the resource it is about, which is what a rule
    /// filtering on `resources` reads.
    pub resource: String,
    /// WHEN the job moved, in epoch milliseconds — the instant the stored job records the
    /// transition at, not the instant the queue is drained.
    pub at: i64,
    /// The rendered `detail` of the event.
    pub detail: ::serde_json::Value,
}

/// What has been published about each job, and what is waiting to be published.
///
/// Held in the store beside the jobs, so the diff is taken against what was last PUBLISHED
/// rather than against whatever a caller last read.
#[derive(Debug, Default)]
pub struct JobEventLog {
    /// The `BackupJobState` last published for each backup job, by `BackupJobId`.
    backup_jobs: BTreeMap<String, String>,
    /// The `CopyJobState` last published for each copy job, by `CopyJobId`.
    copy_jobs: BTreeMap<String, String>,
    /// The `RestoreJobStatus` last published for each restore job, by `RestoreJobId`.
    restore_jobs: BTreeMap<String, String>,
    /// The `RecoveryPointStatus` last published for each stored recovery point, by
    /// `RecoveryPointArn`.
    recovery_points: BTreeMap<String, String>,
    /// The changes queued for the next drain, oldest first. Written by every recorder of
    /// a change, this module's and [`super::plan_events`]'s, through [`enqueue`].
    pub(super) pending: VecDeque<JobStateChange>,
}

/// The single clock pump of one store: whether one is running, which claim it holds and
/// the task it runs as. See [`drive_job_clock`].
#[derive(Debug, Default)]
pub struct JobClock {
    /// Whether a pump is already waiting on this store's due instants.
    running: bool,
    /// The generation of the claim the running pump holds, so a pump releases its own claim
    /// and never a later pump's.
    generation: u64,
    /// The running pump's task, kept so the pump it replaces is aborted rather than merely
    /// forgotten.
    pump: Option<::tokio::task::JoinHandle<()>>,
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Queue one event for every stored job state — and every stored recovery point
    /// status — that differs from the state last published for it, and take that state to
    /// be the published one.
    ///
    /// This is the whole of the "which transitions owe an event" rule: a state the stored
    /// record carries is compared against the last one published, so every move — the
    /// settle the clock owed, and any later transition — owes exactly one event, and a
    /// state that has not changed owes none however many times this runs. A record seen for
    /// the FIRST time only seeds the snapshot: the state its start call stored is what that
    /// call's own response reports, not a move (see the module docs).
    ///
    /// Call it under the store lock, after whatever the operation materialised, and then
    /// [`publish_job_state_changes`] with the lock released.
    pub fn record_job_state_changes(&mut self) {
        // The jobs, the snapshots and the queue are DISJOINT fields of the store, so a
        // stored job is read while the snapshot and the queue are written.
        let Self {
            backup_jobs,
            copy_jobs,
            restore_jobs,
            recovery_points,
            job_events,
            ..
        } = self;
        for (job_id, job) in backup_jobs.iter() {
            let published = job_events.backup_jobs.get(job_id).cloned();
            if published.as_deref() == Some(job.state.as_str()) {
                continue;
            }
            job_events
                .backup_jobs
                .insert(job_id.clone(), job.state.clone());
            if published.is_some() {
                enqueue(&mut job_events.pending, backup_job_change(job));
            }
        }
        for (copy_job_id, job) in copy_jobs.iter() {
            let published = job_events.copy_jobs.get(copy_job_id).cloned();
            if published.as_deref() == Some(job.state.as_str()) {
                continue;
            }
            job_events
                .copy_jobs
                .insert(copy_job_id.clone(), job.state.clone());
            if published.is_some() {
                enqueue(&mut job_events.pending, copy_job_change(job));
            }
        }
        for (restore_job_id, job) in restore_jobs.iter() {
            let published = job_events.restore_jobs.get(restore_job_id).cloned();
            if published.as_deref() == Some(job.status.as_str()) {
                continue;
            }
            job_events
                .restore_jobs
                .insert(restore_job_id.clone(), job.status.clone());
            if published.is_some() {
                enqueue(&mut job_events.pending, restore_job_change(job));
            }
        }
        // A recovery point moves on a clock of its own — the `CREATING` -> `COMPLETED` the
        // backup that wrote it owes, and whatever a later call makes of it — and each of
        // those moves is announced the same way a job's is, under the detail-type of the
        // KIND of thing that moved ([`RECOVERY_POINT_STATE_CHANGE`]). The point a start
        // call stored is seen here for the first time and only seeds the snapshot, for the
        // same reason a job's stored state does: it is what that call wrote, not a move.
        for (arn, point) in recovery_points.iter() {
            let published = job_events.recovery_points.get(arn).cloned();
            if published.as_deref() == Some(point.status.as_str()) {
                continue;
            }
            job_events
                .recovery_points
                .insert(arn.clone(), point.status.clone());
            if published.is_some() {
                enqueue(&mut job_events.pending, recovery_point_change(point));
            }
        }
        // A snapshot belongs to a job: one the store no longer holds can never differ
        // again, so the snapshots do not grow past the jobs they are about.
        job_events
            .backup_jobs
            .retain(|job_id, _| backup_jobs.contains_key(job_id));
        job_events
            .recovery_points
            .retain(|arn, _| recovery_points.contains_key(arn));
        job_events
            .copy_jobs
            .retain(|job_id, _| copy_jobs.contains_key(job_id));
        job_events
            .restore_jobs
            .retain(|job_id, _| restore_jobs.contains_key(job_id));
    }

    /// Take at most `taking` queued changes out of the store, oldest first — one batch of
    /// the drain [`publish_job_state_changes`] repeats until the queue is empty.
    fn take_pending_job_events(&mut self, taking: usize) -> Vec<JobStateChange> {
        let taking = taking.min(self.job_events.pending.len());
        self.job_events.pending.drain(..taking).collect()
    }

    /// The earliest instant at which this store owes a job transition, if any.
    ///
    /// Only a job in a state the clock still owes a move out of has a due instant: one
    /// that has already settled owes nothing however long ago its instant passed.
    fn next_job_due_at(&self) -> Option<SystemTime> {
        let backups = self
            .backup_jobs
            .values()
            .filter(|job| super::backup_job_awaits_the_clock(&job.state))
            .map(|job| job.settles_at);
        let copies = self
            .copy_jobs
            .values()
            .filter(|job| job.state == super::COPY_JOB_STATE_CREATED)
            .map(|job| job.settles_at);
        // A restore job owes its terminal status once the restore it started has created
        // the resource it was asked for: until then the completion is owed by the restore
        // rather than by the clock, so there is nothing for the pump to wait for and the
        // restore itself wakes it ([`drive_clock`]) when it records that resource.
        let restores = self
            .restore_jobs
            .values()
            .filter(|job| {
                job.status == super::RESTORE_JOB_STATUS_PENDING
                    && job.created_resource_arn.is_some()
            })
            .map(|job| job.settles_at);
        // A recovery point still being written owes its own `CREATING` -> `COMPLETED`
        // transition at the instant the point itself records, which is what makes that
        // move due with no call to read it.
        let points = self
            .recovery_points
            .values()
            .filter(|point| point.status == super::RECOVERY_POINT_STATUS_CREATING)
            .map(|point| point.settles_at);
        backups.chain(copies).chain(restores).chain(points).min()
    }

    /// Materialise every job transition the clock has already made due, and queue the
    /// events those transitions owe.
    ///
    /// The SAME settle steps a read of one job makes, applied to the whole store, so a
    /// transition the pump materialises and one a reader materialises are the same write:
    /// a job's state and completion instant do not depend on who looked first.
    fn materialize_due_job_transitions(&mut self, now: SystemTime) {
        self.settle_jobs(now);
        self.settle_copy_jobs(now);
        self.settle_restore_jobs(now);
        // The recovery points a backup or a copy wrote settle on the same clock, and a
        // read of one materialises exactly this step
        // ([`super::ComAmazonawsBackupCryocontrollerusermanagerState::settle_recovery_points`]),
        // so a point's state does not depend on whether anybody looked at it.
        self.settle_recovery_points(now);
        self.record_job_state_changes();
    }

    /// What the clock pump waits for next, or `None` — having RELEASED its claim — when the
    /// store owes nothing.
    ///
    /// Observing "nothing is owed" and standing down happen in ONE critical section, which
    /// is what makes the hand-over race-free: a start call holds this same lock to store
    /// its job (and so its due instant) BEFORE it tries to claim the pump, so either this
    /// call sees that due instant and the running pump keeps it, or the start's claim finds
    /// the flag cleared and starts a pump of its own. A change already queued but not yet
    /// published keeps the pump awake too, so the tail of a burst does not wait for the
    /// next call.
    fn next_job_due_or_release_pump(
        &mut self,
        now: SystemTime,
        generation: u64,
    ) -> Option<SystemTime> {
        if !self.job_events.pending.is_empty() {
            return Some(now);
        }
        match self.next_job_due_at() {
            Some(due) => Some(due),
            None => {
                self.release_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single clock pump for this store: the claim's GENERATION when the caller
    /// may start one, `None` when one is already running.
    ///
    /// The generation is what makes a release safe: a pump releases its own claim and never
    /// a later pump's (see [`Self::release_clock_pump`]).
    fn start_clock_pump(&mut self) -> Option<u64> {
        if self.job_clock.running {
            return None;
        }
        self.job_clock.running = true;
        self.job_clock.generation = self.job_clock.generation.wrapping_add(1);
        Some(self.job_clock.generation)
    }

    /// Release the claim `generation` took — and ONLY that one.
    ///
    /// A pump that ends on the wake-up cap must not clear a claim a pump started since is
    /// holding, or two pumps would run against one store.
    fn release_clock_pump(&mut self, generation: u64) {
        if self.job_clock.generation == generation {
            self.job_clock.running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces — but only while
    /// `generation` is still the LIVE claim.
    ///
    /// The handle is stored after the task is spawned, and a pump that ran out of work
    /// releases its claim before this runs, so a start call may have claimed and stored its
    /// own pump in between: storing this handle then would abort that LIVE pump and leave
    /// the claim held by nobody. A handle whose claim has moved on belongs to a task that
    /// has already ended, so it is simply dropped. While the claim IS this pump's, the
    /// replaced handle is aborted rather than merely forgotten, which is what guarantees no
    /// earlier pump task runs beside the one holding the claim.
    fn set_clock_pump(&mut self, generation: u64, handle: ::tokio::task::JoinHandle<()>) {
        if self.job_clock.generation != generation {
            return;
        }
        if let Some(replaced) = self.job_clock.pump.replace(handle) {
            replaced.abort();
        }
    }
}

/// The shortest wait one wake-up of the clock pump makes.
///
/// A due instant already in the past is materialised on the NEXT wake-up, not at this
/// one's instant, so the pump always hands the runtime back and a wake-up that materialises
/// nothing cannot turn the loop into a spin. It is far below the span a job settles after
/// (`super::JOB_SETTLE_AFTER`), so it never delays a transition noticeably.
const MIN_CLOCK_WAIT: ::std::time::Duration = ::std::time::Duration::from_millis(5);

/// The longest wait one wake-up of the clock pump makes before it looks at the store again.
///
/// Every transition this service owes falls due a fixed span after the call that started
/// it, so a wait is never near this value; it is a backstop, so a due instant far in the
/// future cannot park the pump indefinitely.
const MAX_CLOCK_WAIT: ::std::time::Duration = ::std::time::Duration::from_secs(5);

/// How many wake-ups one pump makes before it stands down.
///
/// A backstop, not a budget: the transitions the store owes are already finite (each job
/// makes one, and the stores are capped by `super::MAX_BACKUP_JOBS` and
/// `super::MAX_COPY_JOBS`), and every wake-up either materialises a transition or clears
/// the due instant that woke it. The cap guarantees termination even if a later change left
/// a due instant that materialising does not clear, and the next start call begins a fresh
/// pump.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

/// Publish the job state changes the CLOCK owes, as they fall due, with no further call
/// from anybody.
///
/// This is what makes the event path worth having: a client that reacts to a backup or a
/// copy through EventBridge makes no further AWS Backup call, so a transition published
/// only by the next reader would never be published at all. This starts ONE in-process task
/// that waits for the earliest due instant the store holds, materialises what is then due
/// ([`ComAmazonawsBackupCryocontrollerusermanagerState`]'s own
/// `materialize_due_job_transitions`,
/// the same settle steps every read makes) and publishes what that produced, until nothing
/// is owed.
///
/// It does not change WHEN a transition happens: only what the stored due instant has
/// already made due is applied, so a caller reading before that instant still sees the
/// transient state, and two reads at the same instant still answer identically. The task
/// waits for the clock so that no request ever has to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// `MAX_CLOCK_WAKE_UPS` wake-ups, and it exits as soon as the store owes nothing. With no
/// async runtime to spawn onto there is no pump, and the transitions are materialised and
/// published by the next operation instead. It needs no Region of its own: each change it
/// publishes names the Region its own job belongs to ([`JobStateChange::region`]), so the
/// one pump serves every Region's jobs.
pub fn drive_job_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    drive_clock(ctx.callback(), &state);
}

/// [`drive_job_clock`] over the store itself, for a task that already holds this service's
/// state rather than the engine it lives in.
///
/// The restore a start performs as its own task calls this once it has recorded the resource
/// it created: that write is what makes the job's terminal status due, and the write happens
/// under the SAME lock a pump takes to decide whether it still has work — so either the
/// running pump sees the due instant and keeps it, or it has already stood down and this
/// claim starts a pump of its own.
pub fn drive_clock(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsBackupCryocontrollerusermanagerState>,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_clock_pump() else {
        return;
    };
    let callback = callback.clone();
    let pumped = ::engine::ServiceState::clone(state);
    let state = ::engine::ServiceState::clone(state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that ran out
        // of work has already released its claim inside the one critical section that
        // observed it (`next_job_due_or_release_pump`).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = state.lock().next_job_due_or_release_pump(now, generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps
            // the backstop above from ever costing a CPU-bound spin, and it hands the
            // runtime back between wake-ups.
            let wait = due
                .duration_since(SystemTime::now())
                .unwrap_or_default()
                .clamp(MIN_CLOCK_WAIT, MAX_CLOCK_WAIT);
            ::tokio::time::sleep(wait).await;
            state
                .lock()
                .materialize_due_job_transitions(SystemTime::now());
            publish_queued(&callback, &state).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            state.lock().release_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten,
    // and only while this pump's claim is still the live one.
    pumped.lock().set_clock_pump(generation, handle);
}

/// Append `change` to the queue, dropping the oldest once the queue is full.
pub(super) fn enqueue(pending: &mut VecDeque<JobStateChange>, change: JobStateChange) {
    while pending.len() >= MAX_PENDING_EVENTS {
        pending.pop_front();
        ::tracing::warn!(
            "the queue of job state changes waiting to be published to EventBridge is full \
             at {MAX_PENDING_EVENTS}; the oldest change was dropped"
        );
    }
    pending.push_back(change);
}

/// The change a BACKUP job's move owes: the job's own stored members as the event `detail`,
/// and the recovery point it wrote as the resource the event is about.
fn backup_job_change(job: &BackupJob) -> JobStateChange {
    let mut detail = ::serde_json::Map::new();
    detail.insert("backupJobId".to_string(), job.job_id.clone().into());
    detail.insert("state".to_string(), job.state.clone().into());
    detail.insert(
        "backupVaultName".to_string(),
        job.backup_vault_name.clone().into(),
    );
    detail.insert(
        "backupVaultArn".to_string(),
        job.backup_vault_arn.clone().into(),
    );
    detail.insert(
        "recoveryPointArn".to_string(),
        job.recovery_point_arn.clone().into(),
    );
    detail.insert("resourceArn".to_string(), job.resource_arn.clone().into());
    detail.insert("iamRoleArn".to_string(), job.iam_role_arn.clone().into());
    detail.insert("percentDone".to_string(), job.percent_done.clone().into());
    detail.insert(
        "backupSizeInBytes".to_string(),
        job.backup_size_in_bytes.into(),
    );
    detail.insert(
        "bytesTransferred".to_string(),
        job.bytes_transferred.into(),
    );
    insert_instant(&mut detail, "creationDate", Some(job.creation_date));
    insert_instant(&mut detail, "completionDate", job.completion_date);
    insert_instant(&mut detail, "startBy", job.start_by);
    // A member the stored job does not carry is absent from the detail rather than
    // reported as null, exactly as it is absent from a read of the same job.
    if let Some(resource_type) = job.resource_type.as_ref() {
        detail.insert("resourceType".to_string(), resource_type.clone().into());
    }
    if let Some(status_message) = job.status_message.as_ref() {
        detail.insert("statusMessage".to_string(), status_message.clone().into());
    }
    // The CLASS the job's outcome falls in, which the moved record's own API reports beside
    // its message (`MessageCategory` of `DescribeBackupJob`): a consumer branching on a
    // failed backup reads WHICH KIND of failure it was off the envelope rather than by
    // calling the control plane again, and the job records it exactly when it records the
    // message.
    if let Some(message_category) = job.message_category.as_ref() {
        detail.insert(
            "messageCategory".to_string(),
            message_category.clone().into(),
        );
    }
    JobStateChange {
        detail_type: BACKUP_JOB_STATE_CHANGE,
        // A backup job belongs to the vault it backs up INTO, so that vault's Region is
        // the one its events are published in.
        region: vault_region(&job.backup_vault_arn),
        resource: job.recovery_point_arn.clone(),
        at: epoch_millis(moved_at(job.completion_date, job.creation_date)),
        detail: ::serde_json::Value::Object(detail),
    }
}

/// The change a COPY job's move owes. Both ends of the copy are in the detail — the vault
/// and recovery point the copy was read from, and the vault and recovery point it was
/// written to — read off the stored job, so a copy whose destination vault lives in another
/// region reports that vault's own ARN, the SAME value a read of the job reports for it.
fn copy_job_change(job: &CopyJobRecord) -> JobStateChange {
    let mut detail = ::serde_json::Map::new();
    detail.insert("copyJobId".to_string(), job.copy_job_id.clone().into());
    detail.insert("state".to_string(), job.state.clone().into());
    detail.insert(
        "sourceBackupVaultArn".to_string(),
        job.source_backup_vault_arn.clone().into(),
    );
    detail.insert(
        "sourceRecoveryPointArn".to_string(),
        job.source_recovery_point_arn.clone().into(),
    );
    detail.insert(
        "destinationBackupVaultArn".to_string(),
        job.destination_backup_vault_arn.clone().into(),
    );
    detail.insert(
        "destinationRecoveryPointArn".to_string(),
        job.destination_recovery_point_arn.clone().into(),
    );
    detail.insert("resourceArn".to_string(), job.resource_arn.clone().into());
    detail.insert("iamRoleArn".to_string(), job.iam_role_arn.clone().into());
    detail.insert(
        "backupSizeInBytes".to_string(),
        job.backup_size_in_bytes.into(),
    );
    insert_instant(&mut detail, "creationDate", Some(job.creation_date));
    insert_instant(&mut detail, "completionDate", job.completion_date);
    if let Some(resource_type) = job.resource_type.as_ref() {
        detail.insert("resourceType".to_string(), resource_type.clone().into());
    }
    if let Some(status_message) = job.status_message.as_ref() {
        detail.insert("statusMessage".to_string(), status_message.clone().into());
    }
    JobStateChange {
        detail_type: COPY_JOB_STATE_CHANGE,
        // A copy job runs where it was started, which is the Region of the vault it reads
        // FROM: the destination vault may be in another Region, and the copy's own events
        // are not that Region's.
        region: vault_region(&job.source_backup_vault_arn),
        resource: job.destination_recovery_point_arn.clone(),
        at: epoch_millis(moved_at(job.completion_date, job.creation_date)),
        detail: ::serde_json::Value::Object(detail),
    }
}

/// The change a RESTORE job's move owes.
///
/// The restore's own two ends are in the detail — the recovery point it reads FROM and the
/// resource it CREATED — read off the stored job, so every value is the one
/// `DescribeRestoreJob` reports for the same job: a member the job does not carry (the
/// created resource of a restore that has created none, a status message) is absent from
/// the detail exactly as it is absent from that read.
fn restore_job_change(job: &RestoreJobRecord) -> JobStateChange {
    let mut detail = ::serde_json::Map::new();
    detail.insert("restoreJobId".to_string(), job.restore_job_id.clone().into());
    detail.insert("status".to_string(), job.status.clone().into());
    detail.insert(
        "recoveryPointArn".to_string(),
        job.recovery_point_arn.clone().into(),
    );
    detail.insert(
        "sourceResourceArn".to_string(),
        job.source_resource_arn.clone().into(),
    );
    detail.insert(
        "backupVaultArn".to_string(),
        job.backup_vault_arn.clone().into(),
    );
    // The one reading of how much of a restore is done, shared with the read of the same
    // job, so the event and the read never disagree about it.
    detail.insert("percentDone".to_string(), job.percent_done().into());
    insert_instant(&mut detail, "creationDate", Some(job.creation_date));
    insert_instant(&mut detail, "completionDate", job.completion_date);
    if let Some(created_resource_arn) = job.created_resource_arn.as_ref() {
        detail.insert(
            "createdResourceArn".to_string(),
            created_resource_arn.clone().into(),
        );
    }
    if let Some(iam_role_arn) = job.iam_role_arn.as_ref() {
        detail.insert("iamRoleArn".to_string(), iam_role_arn.clone().into());
    }
    if let Some(resource_type) = job.resource_type.as_ref() {
        detail.insert("resourceType".to_string(), resource_type.clone().into());
    }
    if let Some(status_message) = job.status_message.as_ref() {
        detail.insert("statusMessage".to_string(), status_message.clone().into());
    }
    JobStateChange {
        detail_type: RESTORE_JOB_STATE_CHANGE,
        // A restore belongs to the vault holding the point it restores FROM, so that
        // vault's Region is the one its events are published in.
        region: vault_region(&job.backup_vault_arn),
        // The recovery point the restore is OF: the one ARN the job carries for every
        // state it is in, where the resource it creates exists only once it has been
        // created.
        resource: job.recovery_point_arn.clone(),
        at: epoch_millis(moved_at(job.completion_date, job.creation_date)),
        detail: ::serde_json::Value::Object(detail),
    }
}

/// The change a RECOVERY POINT's move owes: the point's own stored members as the event
/// `detail`, and the point ITSELF as the resource the event is about.
///
/// Every member is the one a read of the same point reports for it
/// (`DescribeRecoveryPoint`, `ListRecoveryPointsByBackupVault`), under that member's name
/// with a lower-case first letter as an event detail spells it — so the point a client
/// reacts to on the bus and the point it then reads back cannot disagree. A member the
/// stored point does not carry is absent from the detail rather than reported as null,
/// exactly as it is absent from that read.
///
/// That holds for the moved-into state as well, which is why this detail spells it
/// `status` where a JOB's spells it `state`: what a recovery point moves through is its
/// `Status`, of the enum `RecoveryPointStatus`
/// (`smithy_query(query="operation", operation="DescribeRecoveryPoint")`), where a backup
/// or a copy job moves through a `State`. The kind of record that moved decides the member
/// name, so a client filtering on `detail.status` for a point and on `detail.state` for a
/// job reads what each record's own API reports.
fn recovery_point_change(point: &RecoveryPoint) -> JobStateChange {
    let mut detail = ::serde_json::Map::new();
    detail.insert("recoveryPointArn".to_string(), point.arn.clone().into());
    detail.insert("status".to_string(), point.status.clone().into());
    detail.insert(
        "backupVaultName".to_string(),
        point.backup_vault_name.clone().into(),
    );
    detail.insert(
        "backupVaultArn".to_string(),
        point.backup_vault_arn.clone().into(),
    );
    detail.insert("resourceArn".to_string(), point.resource_arn.clone().into());
    detail.insert("iamRoleArn".to_string(), point.iam_role_arn.clone().into());
    detail.insert(
        "backupSizeInBytes".to_string(),
        point.backup_size_in_bytes.into(),
    );
    detail.insert("isEncrypted".to_string(), point.is_encrypted.into());
    detail.insert("isParent".to_string(), point.is_parent.into());
    insert_instant(&mut detail, "creationDate", Some(point.creation_date));
    insert_instant(&mut detail, "completionDate", point.completion_date);
    if let Some(resource_type) = point.resource_type.as_ref() {
        detail.insert("resourceType".to_string(), resource_type.clone().into());
    }
    if let Some(encryption_key_arn) = point.encryption_key_arn.as_ref() {
        detail.insert(
            "encryptionKeyArn".to_string(),
            encryption_key_arn.clone().into(),
        );
    }
    if let Some(index_status) = point.index_status.as_ref() {
        detail.insert("indexStatus".to_string(), index_status.clone().into());
    }
    JobStateChange {
        detail_type: RECOVERY_POINT_STATE_CHANGE,
        // A recovery point belongs to the vault that HOLDS it, so that vault's Region is
        // the one its events are published in — the destination vault's own Region for a
        // point a cross-Region copy wrote.
        region: vault_region(&point.backup_vault_arn),
        resource: point.arn.clone(),
        at: epoch_millis(moved_at(point.completion_date, point.creation_date)),
        detail: ::serde_json::Value::Object(detail),
    }
}

/// The Region the vault `vault_arn` names belongs to.
///
/// Read off the STORED ARN, so the bus a job's events reach is a property of the job rather
/// than of whichever call or clock tick happened to publish it. An ARN that is not a
/// vault's — nothing this service stores leaves one — falls back to the Region an unsigned
/// request is served in (`super::DEFAULT_REGION`), so a change is still published
/// somewhere rather than dropped.
fn vault_region(vault_arn: &str) -> String {
    super::backup_vault_arn_parts(vault_arn)
        .map(|parts| parts.region)
        .unwrap_or_else(|| super::DEFAULT_REGION.to_string())
}

/// WHEN the job moved: the instant the stored job records for the state it is now in — its
/// completion instant once it has one, and otherwise the instant it was created. Read off
/// the job, so the event's `time` is the instant the transition happened rather than the
/// instant the queue was drained.
fn moved_at(completion_date: Option<SystemTime>, creation_date: SystemTime) -> SystemTime {
    completion_date.unwrap_or(creation_date)
}

/// `time` as the epoch milliseconds an event's `time` carries.
pub(super) fn epoch_millis(time: SystemTime) -> i64 {
    super::modelled_timestamp(time)
        .to_millis()
        .unwrap_or_default()
}

/// Insert `at` under `member` as the RFC 3339 instant AWS's event details carry, when there
/// is an instant to report and it can be expressed as one.
pub(super) fn insert_instant(
    detail: &mut ::serde_json::Map<String, ::serde_json::Value>,
    member: &str,
    at: Option<SystemTime>,
) {
    let rendered = at.and_then(|at| {
        super::modelled_timestamp(at)
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .ok()
    });
    if let Some(rendered) = rendered {
        detail.insert(member.to_string(), rendered.into());
    }
}

/// Publish every state change this store has queued onto the account's default
/// EventBridge bus, and leave the queue empty.
///
/// Every recorder's changes are drained here — a job's and a recovery point's moves, and
/// the mutations of a backup plan ([`super::plan_events`]) — so this service publishes
/// through one path.
///
/// Called by a handler AFTER it has released the store's lock, so no lock is held across
/// the await: each batch is taken out in one critical section, so nothing is published
/// twice, and the in-process `PutEvents` runs outside it. The queue is drained until it is
/// EMPTY — a batch is capped (`MAX_PUBLISHED_PER_DRAIN`) so no single request is
/// unbounded, and the loop is what guarantees the tail of a burst is published by the call
/// that produced it rather than waiting for the next one.
///
/// Each change is published in the Region its own job belongs to
/// ([`JobStateChange::region`]), so which call or clock tick drains the queue decides
/// nothing about where an event arrives.
///
/// Nothing about the answer changes what the caller of the operation is told: publication
/// is best effort, and an emulator with no EventBridge mounted has nowhere to publish, so a
/// failure is logged and the operation's own result stands.
pub async fn publish_job_state_changes<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
) {
    publish_pending(ctx.callback(), engine).await;
}

/// [`publish_job_state_changes`] over the callback alone, so the clock pump
/// ([`drive_job_clock`]) — which outlives the request that started it and has no
/// `OperationContext` — publishes through exactly the same path a handler does.
async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    publish_queued(callback, &state).await;
}

/// [`publish_pending`] over the store itself, for a task that already holds this service's
/// state rather than the engine it lives in — the restore a start performs as its own task
/// records what it came to on the job and publishes that move through this same path.
pub async fn publish_queued(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsBackupCryocontrollerusermanagerState>,
) {
    // The guard is taken and dropped inside this statement.
    let mut changes = state.lock().take_pending_job_events(MAX_PUBLISHED_PER_DRAIN);
    if changes.is_empty() {
        return;
    }
    // One client per Region a batch carries changes for, built on first use and then
    // reused: a batch is usually one Region's, and two are two buses.
    let mut clients: BTreeMap<String, ::aws_sdk_eventbridge::Client> = BTreeMap::new();
    while !changes.is_empty() {
        let mut by_region: BTreeMap<&str, Vec<&JobStateChange>> = BTreeMap::new();
        for change in &changes {
            by_region
                .entry(change.region.as_str())
                .or_default()
                .push(change);
        }
        for (region, published) in by_region {
            if !clients.contains_key(region) {
                clients.insert(
                    region.to_string(),
                    // The ONE construction of the in-process EventBridge client every
                    // emulated service publishes through, so this hop is configured in one
                    // place.
                    ::service_common::eventbridge_client::eventbridge_client(callback, region),
                );
            }
            if let Some(client) = clients.get(region) {
                publish_batch(client, &published).await;
            }
        }
        // Whatever a concurrent call left behind: the loop ends on the empty queue, which
        // every batch brings closer because a batch is only ever removed from it.
        changes = state.lock().take_pending_job_events(MAX_PUBLISHED_PER_DRAIN);
    }
}

/// Publish one batch of changes, in `PutEvents` requests of at most
/// [`MAX_PUT_EVENTS_ENTRIES`] entries.
async fn publish_batch(client: &::aws_sdk_eventbridge::Client, changes: &[&JobStateChange]) {
    for window in changes.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for change in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(change.detail_type)
                // The event's `time` is WHEN the job moved, not when the queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the event,
                // per RFC3339. If no time stamp is provided, the time stamp of the
                // PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    change.at,
                ))
                .resources(&change.resource)
                .detail(change.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an AWS Backup job state change could not be published to EventBridge"
            );
        }
    }
}
