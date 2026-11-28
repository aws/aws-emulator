// SPDX-License-Identifier: CC0-1.0

//! Cancelling a backup job: what a stop does to the job it names, and the conditions it
//! refuses.
//!
//! # The rule this module implements
//!
//! A stop "attempts to cancel a job to create a one-time backup of a resource" (the model
//! doc for `StopBackupJob`), so it acts on a job that has NOT come to rest: such a job stops
//! heading for the backup it was taking and heads for the terminal `ABORTED` state instead,
//! and the recovery point that backup was writing is not left behind — a cancelled backup
//! produces none, exactly as a job whose backup cannot be performed at all stores none
//! (`super::job_outcome`). A job that has ALREADY reached a terminal state has nothing left
//! to cancel, and an id this account holds no job under names nothing to cancel either; each
//! is refused with the class AWS answers for its condition ([`JobStopRefusal`]), and nothing
//! about the stored job changes.
//!
//! The cancellation is not instantaneous. The job reports `ABORTING` until the clock makes
//! the `ABORTED` it is owed due — the same span every other transition of this service falls
//! due after (`super::JOB_SETTLE_AFTER`) — so a caller that reads the job straight after
//! stopping it observes that transient state, no handler sleeps for it, and the move is
//! materialised by whoever looks first and published as every other job move is
//! (`super::events`).
//!
//! The RESOURCE TYPES the operation is not supported for are the other refusal: its model doc
//! lists Amazon Aurora, Amazon DocumentDB, Amazon FSx (every file-system flavour), Amazon
//! Neptune, SAP HANA databases on Amazon EC2 instances and Amazon RDS, so a job taken of a
//! resource of one of those types is refused rather than reported as cancelled
//! ([`STOP_UNSUPPORTED_RESOURCE_TYPES`]).

use std::time::SystemTime;

use super::{
    backup_vault_arn_parts, ComAmazonawsBackupCryocontrollerusermanagerState,
    InvalidRequestException, ResourceNotFoundException, BACKUP_JOB_STATE_CREATED, JOB_SETTLE_AFTER,
};

/// `BackupJobState` of a job whose cancellation has been accepted and is under way (model
/// enum `BackupJobState`, `ABORTED | ABORTING | COMPLETED | CREATED | EXPIRED | FAILED |
/// PARTIAL | PENDING | RUNNING`).
pub const BACKUP_JOB_STATE_ABORTING: &str = "ABORTING";

/// `BackupJobState` a cancelled job comes to rest in (same enum).
pub const BACKUP_JOB_STATE_ABORTED: &str = "ABORTED";

/// The `BackupJobState` values a job this service stores can still be moved OUT of: the one a
/// start stores and the one a stop moves it into. Every other value of the model's enum is a
/// state a job has come to rest in, which is why the clock owes a move only out of these two
/// and a stop cancels only a job in one of them.
pub const UNSETTLED_BACKUP_JOB_STATES: &[&str] =
    &[BACKUP_JOB_STATE_CREATED, BACKUP_JOB_STATE_ABORTING];

/// The Backup resource types a stop is not supported for, as the model doc for
/// `StopBackupJob` lists the services it names ("this action is not supported for the
/// following services"), spelled as this service's own resource types
/// (`super::resource_type_of`).
///
/// SAP HANA databases on Amazon EC2 instances are named by that doc too and are absent here
/// for one reason: the model's `ResourceType` pattern forbids the space in that spelling, so
/// no ARN resolves to it and no stored job can carry it.
pub const STOP_UNSUPPORTED_RESOURCE_TYPES: &[&str] =
    &["Aurora", "DocumentDB", "FSx", "Neptune", "RDS"];

/// The `StatusMessage` a cancelled job carries, which is what it ended for: the caller asked
/// the service to stop it, rather than anything about the backup itself. The FIXED sentence
/// AWS states for that class — it names no part of the request.
pub const MESSAGE_BACKUP_JOB_STOPPED: &str = "Backup job was stopped by user.";

/// Why a stop cannot cancel the job it named, in the class AWS answers for each condition.
///
/// Two classes, because they are two different facts about the request: an id naming no job
/// of this account is the NOT-FOUND condition ([`stop_backup_job_not_found`]), while a job
/// that exists but cannot be cancelled is a fact about the REQUEST's fit to the stored job,
/// which is the modelled `InvalidRequestException`. The operation lifts each into its own
/// modelled error variant.
#[derive(Debug, Clone)]
pub enum JobStopRefusal {
    /// This account holds no backup job under the id the request named.
    Absent(ResourceNotFoundException),
    /// The job exists, and nothing about it can be cancelled: it has come to rest, or it is a
    /// backup of a resource type the operation does not support.
    Refused(InvalidRequestException),
}

/// The sentence AWS states for "the backup job this stop names is not one this account
/// holds" — FIXED: it names no part of the request, so the id asked about does not appear in
/// it.
///
/// A stop words this condition its OWN way, which is why this is not the sentence a READ of a
/// backup job states (`super::MESSAGE_BACKUP_JOB_NOT_FOUND`): the two calls resolve the job in
/// different places and AWS answers each with its own wording. No `Code`: this condition
/// carries no observed token on this operation, where the read's does
/// (`super::ERROR_CODE_BACKUP_JOB_NOT_FOUND`).
pub const MESSAGE_STOP_BACKUP_JOB_NOT_FOUND: &str = "BackupJob not found";

/// The modelled refusal of a stop naming an id this account holds no backup job under.
#[must_use]
pub fn stop_backup_job_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MESSAGE_STOP_BACKUP_JOB_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal of a stop of a job that has already come to rest.
///
/// The sentence AWS states for this condition is about the move the stop asked for rather
/// than about the job: it names the state the stop would have moved the job INTO
/// ([`BACKUP_JOB_STATE_ABORTING`]) and no part of the request, so a job that has come to rest
/// is told the cancellation cannot be applied.
#[must_use]
pub fn backup_job_not_cancellable() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(format!(
            "Cannot update the backup job to {BACKUP_JOB_STATE_ABORTING} state"
        )),
        ..Default::default()
    }
}

/// The modelled refusal of a stop of a job whose protected resource is of a type the
/// operation does not support, naming that type — read from the stored job
/// ([`STOP_UNSUPPORTED_RESOURCE_TYPES`]).
#[must_use]
pub fn stop_unsupported_for_resource_type(resource_type: &str) -> InvalidRequestException {
    InvalidRequestException {
        message: Some(format!(
            "StopBackupJob is not supported for resource type {resource_type}"
        )),
        ..Default::default()
    }
}

/// Whether the clock still owes the job in `state` a move, which is what makes it a job a stop
/// can cancel ([`UNSETTLED_BACKUP_JOB_STATES`]).
#[must_use]
pub fn backup_job_awaits_the_clock(state: &str) -> bool {
    UNSETTLED_BACKUP_JOB_STATES.contains(&state)
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Cancel the backup job `job_id` names, or state why it cannot be cancelled.
    ///
    /// The ONE place the cancellation is applied, so the job's state, the recovery point its
    /// backup was writing and the instant its `ABORTED` falls due are written together.
    ///
    /// What it does, in the order the decisions are taken:
    ///
    /// 1. materialises the transition the clock ALREADY owed this job, since a job whose
    ///    settle instant has passed reached its terminal state at that instant whether or not
    ///    anybody called — so the stop is judged against the state the job is really in;
    /// 2. refuses an id no stored job answers to, and a job that cannot be cancelled;
    /// 3. moves the job to `ABORTING` with the `ABORTED` it is owed due a span later, records
    ///    why it ended, and removes the recovery point the cancelled backup was writing, so no
    ///    vault is left holding a point of a backup that was cancelled.
    ///
    /// A stop of a job whose cancellation is already under way cancels the same job again: it
    /// is accepted and changes nothing, so a repeated stop neither pushes the due instant back
    /// nor reports a second cancellation.
    pub fn stop_backup_job(
        &mut self,
        job_id: &str,
        now: SystemTime,
    ) -> Result<(), JobStopRefusal> {
        self.settle_job(job_id, now);
        let Some(job) = self.backup_jobs.get(job_id) else {
            return Err(JobStopRefusal::Absent(stop_backup_job_not_found()));
        };
        if let Some(resource_type) = job
            .resource_type
            .as_deref()
            .filter(|resource_type| STOP_UNSUPPORTED_RESOURCE_TYPES.contains(resource_type))
        {
            return Err(JobStopRefusal::Refused(stop_unsupported_for_resource_type(
                resource_type,
            )));
        }
        if !backup_job_awaits_the_clock(&job.state) {
            return Err(JobStopRefusal::Refused(backup_job_not_cancellable()));
        }
        if job.state == BACKUP_JOB_STATE_ABORTING {
            return Ok(());
        }
        let vault_name = job.backup_vault_name.clone();
        let vault_arn = job.backup_vault_arn.clone();
        let point_arn = job.recovery_point_arn.clone();
        if let Some(job) = self.backup_jobs.get_mut(job_id) {
            job.state = BACKUP_JOB_STATE_ABORTING.to_string();
            // Checked arithmetic, as every other due instant of this service: one this clock
            // cannot express makes the move due at once rather than panicking.
            job.settles_at = now.checked_add(JOB_SETTLE_AFTER).unwrap_or(now);
            job.status_message = Some(MESSAGE_BACKUP_JOB_STOPPED.to_string());
            // The job ends ABORTED, so the terminal state it was owed decides nothing any
            // more: what it ended for is the cancellation above, and a job carrying a reason
            // it did not fail for could report a failure it never had
            // (`super::job_outcome`).
            job.failure = None;
        }
        // A cancelled backup leaves no recovery point: the point this job was writing goes
        // from the vault that held it, so the vault's listing, its `NumberOfRecoveryPoints`
        // and a read of that point all agree the backup produced nothing. The job still
        // reports the `RecoveryPointArn` its start answered with, which resolves to nothing —
        // as the recovery point of a job that produced none does.
        if let Some(parts) = backup_vault_arn_parts(&vault_arn) {
            self.remove_recovery_point(&parts.region, &vault_name, &point_arn);
        }
        Ok(())
    }
}
