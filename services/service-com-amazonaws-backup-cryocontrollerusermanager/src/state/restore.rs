// SPDX-License-Identifier: CC0-1.0

//! The RESTORE side of this service's state: the job `StartRestoreJob` starts,
//! and what every call that names a `RestoreJobId` resolves against.
//!
//! # The rules this module holds
//!
//! * a restore is started from a RECOVERY POINT, so the `RecoveryPointArn` the
//!   request carries is resolved against the stored recovery points
//!   ([`ComAmazonawsBackupCryocontrollerusermanagerState::restorable_recovery_point`])
//!   before anything is persisted, and a value naming none is refused as the
//!   VALUE of that member ([`super::recovery_point_not_found_value`]) — the
//!   parameter class, not the not-found one;
//! * a `RestoreJobId` naming no stored restore job is the OTHER class: the
//!   restore job asked about does not exist
//!   ([`super::restore_job_not_found`]), which is what every call that names one
//!   answers — the read, the metadata read and the validation result alike, so
//!   the two classes stay disjoint whichever call a client makes;
//! * a restore PRODUCES a resource, and that resource belongs to another
//!   service, so an accepted start performs the restore against the service
//!   that owns the protected resource ([`super::perform_restore`]) and reports
//!   as `CreatedResourceArn` the ARN that service answered with — a resource no
//!   service holds is never reported as created, and a restore nothing could
//!   perform ends in the modelled failure status carrying the reason.
//!
//! The stored job is the ONE record `DescribeRestoreJob`,
//! `GetRestoreJobMetadata` and `PutRestoreValidationResult` share, so a
//! validation result one of them records is what the next read reports.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::{ComAmazonawsBackupCryocontrollerusermanagerState, RecoveryPoint};

/// `RestoreJobStatus` of a restore job that has just been started (model enum
/// `RestoreJobStatus`: `PENDING | RUNNING | COMPLETED | ABORTED | FAILED`).
pub const RESTORE_JOB_STATUS_PENDING: &str = "PENDING";

/// `RestoreJobStatus` of a restore job the clock has ended (same enum).
///
/// A restore that AWS finishes is `COMPLETED`, and so is one this emulator
/// performed: the resource it created is held by the service that owns it
/// ([`super::perform_restore`]), so the status reports work that really
/// happened. The clock owes a started job this transition
/// ([`RESTORE_JOB_SETTLE_AFTER`]) so a caller that reads the job straight after
/// starting it still observes the transient status AWS answers with.
pub const RESTORE_JOB_STATUS_COMPLETED: &str = "COMPLETED";

/// `RestoreJobStatus` of a restore job that created no resource (same enum).
///
/// A restore whose resource could not be created — one of a type this emulator
/// performs no restore of, or one the owning service refused — ends here
/// carrying the reason as its `StatusMessage`, rather than being reported
/// `COMPLETED` for a restore that produced nothing.
pub const RESTORE_JOB_STATUS_FAILED: &str = "FAILED";

/// How long after `StartRestoreJob` the stored job reaches its terminal status.
///
/// A CHOSEN span, not a documented one: AWS takes as long as materialising the
/// protected resource needs. It is deliberately non-zero so a caller that reads
/// the job straight after starting it still observes the transient `PENDING`
/// status AWS answers with, and the due instant is stored on the job
/// ([`RestoreJobRecord::settles_at`]) so no handler ever sleeps for it. The
/// same span this service gives the other transitions whose work is not a
/// single store write ([`super::LEGAL_HOLD_SETTLE_AFTER`],
/// [`super::FRAMEWORK_SETTLE_AFTER`]).
pub const RESTORE_JOB_SETTLE_AFTER: Duration = Duration::from_secs(3);

/// `PercentDone` of a restore job that has not ended yet.
pub const RESTORE_JOB_PERCENT_DONE_NONE: &str = "0.0";

/// `PercentDone` of a restore job the clock has ended.
pub const RESTORE_JOB_PERCENT_DONE_COMPLETE: &str = "100.0";

/// The value a read of a restore job's `Metadata` reports in place of every
/// stored value.
///
/// `Metadata` is `@sensitive` in the model, and the pairs it carries are the
/// caller's own configuration of the restore — a target name, a target
/// encryption key, a subnet. A read of a sensitive map reports WHICH keys the
/// restore was configured with, and this fixed sentinel instead of what each
/// one was set to, so the read discloses no value back to anybody who can call
/// the API. The spelling is AWS Backup's own for a masked restore-metadata
/// value.
pub const METADATA_VALUE_MASK: &str = "VALUE_NOT_DISPLAYED";

/// The most restore jobs this emulator keeps.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota, for the reason given on
/// [`super::MAX_VAULTS`]: the store is in memory and every request can add to
/// it. Past the ceiling a start is refused the modelled
/// `InvalidRequestException`, which is also the class AWS answers a restore it
/// will not start in this state with.
pub const MAX_RESTORE_JOBS: usize = 10_000;

/// A restore job: what `StartRestoreJob` starts, `DescribeRestoreJob` reports,
/// `GetRestoreJobMetadata` reports the request's own `Metadata` from, and
/// `PutRestoreValidationResult` records a validation result on.
#[derive(Debug, Clone)]
pub struct RestoreJobRecord {
    pub restore_job_id: String,
    /// The recovery point the restore was asked for, as the resolved stored
    /// point's own ARN — so the job can never name a point the store does not
    /// hold.
    pub recovery_point_arn: String,
    /// The protected resource that recovery point was taken of, which is what
    /// `SourceResourceArn` reports.
    pub source_resource_arn: String,
    /// The ARN of the vault holding that recovery point.
    pub backup_vault_arn: String,
    /// When that recovery point was created, read off the point rather than
    /// stored twice.
    pub recovery_point_creation_date: SystemTime,
    pub iam_role_arn: Option<String>,
    /// The `ResourceType` the restore is of: the one the request named, or the
    /// type of the resolved recovery point when the request named none.
    pub resource_type: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub creation_date: SystemTime,
    /// When the clock owes this job its `PENDING` -> terminal transition. See
    /// [`RESTORE_JOB_SETTLE_AFTER`].
    pub settles_at: SystemTime,
    pub completion_date: Option<SystemTime>,
    /// The `Metadata` the request carried, which is exactly what
    /// `GetRestoreJobMetadata` reports back: "the metadata that you've
    /// specified" for the restore.
    pub metadata: BTreeMap<String, String>,
    pub validation_status: Option<String>,
    pub validation_status_message: Option<String>,
    /// The ARN of the resource the restore created, as the service that owns it
    /// answered with ([`super::RestoreOutcome::Created`]). `None` until the
    /// restore has created one, and for a restore that created none — so this
    /// member never names a resource no service holds.
    pub created_resource_arn: Option<String>,
    /// `CopySourceTagsToRestoredResource`, as the request asked for it.
    pub copy_source_tags_to_restored_resource: bool,
    /// The `IdempotencyToken` the start carried, if any: "retrying a successful
    /// request with the same idempotency token results in a success message
    /// with no action taken" (model doc for the member), so a repeat answers
    /// with this job rather than starting a second one.
    pub idempotency_token: Option<String>,
}

impl RestoreJobRecord {
    /// How much of this restore is done, as `PercentDone` reports it: all of it once the
    /// restore has completed, and none of it otherwise — a restore that created nothing is
    /// not partly done.
    ///
    /// The ONE reading of it, so the read of a job and the event its move publishes never
    /// disagree about how far the same restore got.
    pub fn percent_done(&self) -> &'static str {
        if self.status == RESTORE_JOB_STATUS_COMPLETED {
            RESTORE_JOB_PERCENT_DONE_COMPLETE
        } else {
            RESTORE_JOB_PERCENT_DONE_NONE
        }
    }

    /// The members a LISTING of this job reports, computed from the stored record — the
    /// same members, read off the same fields, that `DescribeRestoreJob` answers with, so a
    /// listed job and a described one can never disagree about one restore.
    ///
    /// The members left unset are the ones nothing in this slice records, for the reasons
    /// the read states: a restore started directly has no restore testing plan behind it
    /// and no parent or nested jobs, no estimate of its remaining time, and no deletion of
    /// the resource it created.
    pub fn listed(&self) -> crate::ops::types::RestoreJobsListMember {
        crate::ops::types::RestoreJobsListMember {
            // The one account whose resources this emulator holds, which is the account
            // the job belongs to.
            account_id: Some(::engine::emulator_account_id().to_string()),
            restore_job_id: Some(self.restore_job_id.clone()),
            recovery_point_arn: Some(self.recovery_point_arn.clone()),
            source_resource_arn: Some(self.source_resource_arn.clone()),
            backup_vault_arn: Some(self.backup_vault_arn.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            completion_date: self
                .completion_date
                .map(::aws_smithy_types::DateTime::from),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            percent_done: Some(self.percent_done().to_string()),
            // The restore moved no bytes, because the backup it reads copied none.
            backup_size_in_bytes: Some(0),
            iam_role_arn: self.iam_role_arn.clone(),
            created_resource_arn: self.created_resource_arn.clone(),
            resource_type: self.resource_type.clone(),
            recovery_point_creation_date: Some(::aws_smithy_types::DateTime::from(
                self.recovery_point_creation_date,
            )),
            validation_status: self.validation_status.clone(),
            validation_status_message: self.validation_status_message.clone(),
            is_parent: false,
            expected_completion_time_minutes: None,
            parent_job_id: None,
            created_by: None,
            deletion_status: None,
            deletion_status_message: None,
        }
    }
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Materialise the transition the clock has already made due, for ONE
    /// restore job: a job whose settle instant has passed, and whose restore
    /// has created the resource it was asked for, finished at that instant
    /// whether or not anybody looked.
    ///
    /// A job whose restore has produced nothing YET keeps the non-terminal
    /// status: the completion it would be reported with is owed by the restore,
    /// not by the clock alone, so nothing is ever reported finished before the
    /// resource it created exists. A restore that created nothing is already
    /// terminal ([`RESTORE_JOB_STATUS_FAILED`]), which this leaves alone.
    pub fn settle_restore_job(&mut self, restore_job_id: &str, now: SystemTime) {
        let Some(job) = self.restore_jobs.get_mut(restore_job_id) else {
            return;
        };
        if now < job.settles_at
            || job.status != RESTORE_JOB_STATUS_PENDING
            || job.created_resource_arn.is_none()
        {
            return;
        }
        job.status = RESTORE_JOB_STATUS_COMPLETED.to_string();
        job.completion_date = Some(job.settles_at);
    }

    /// The same for every stored restore job, for a call that answers about all
    /// of them.
    pub fn settle_restore_jobs(&mut self, now: SystemTime) {
        let ids: Vec<String> = self.restore_jobs.keys().cloned().collect();
        for id in ids {
            self.settle_restore_job(&id, now);
        }
    }

    /// Resolve the restore job a `RestoreJobId` addresses, or the modelled
    /// refusal for one this account does not hold.
    ///
    /// The ONE resolution step every call that names a restore job shares — the
    /// read, the metadata read and the validation result — so none of them can
    /// answer a different class or a different sentence for the same condition.
    pub fn stored_restore_job(
        &self,
        restore_job_id: &str,
    ) -> Result<&RestoreJobRecord, crate::ops::types::ResourceNotFoundException> {
        self.restore_jobs
            .get(restore_job_id)
            .ok_or_else(super::restore_job_not_found)
    }

    /// The same, for a call that WRITES the job it resolves.
    pub fn stored_restore_job_mut(
        &mut self,
        restore_job_id: &str,
    ) -> Result<&mut RestoreJobRecord, crate::ops::types::ResourceNotFoundException> {
        self.restore_jobs
            .get_mut(restore_job_id)
            .ok_or_else(super::restore_job_not_found)
    }

    /// The restore job a repeated `IdempotencyToken` replays, so a retry takes
    /// no second action and answers with the id the first call answered with.
    pub fn restore_job_by_idempotency_token(&self, token: &str) -> Option<&RestoreJobRecord> {
        self.restore_jobs
            .values()
            .find(|job| job.idempotency_token.as_deref() == Some(token))
    }

    /// The stored recovery point a restore can be started from: one this
    /// account holds, in whichever vault holds it, whose backup has settled.
    ///
    /// `StartRestoreJob` names no vault — the recovery point's ARN addresses it
    /// on its own — so the lookup is over every stored point rather than one
    /// vault's index. A point whose backup is still running is deliberately not
    /// one of them, exactly as for every other call that addresses a stored
    /// point ([`ComAmazonawsBackupCryocontrollerusermanagerState::stored_recovery_point`]):
    /// until the backup finishes there is nothing to restore from.
    pub fn restorable_recovery_point(&self, recovery_point_arn: &str) -> Option<&RecoveryPoint> {
        self.recovery_points
            .get(recovery_point_arn)
            .filter(|point| point.status != super::RECOVERY_POINT_STATUS_CREATING)
    }

    /// Materialise the `CREATING` -> `COMPLETED` transition the clock owes
    /// EVERY stored recovery point, for a call that resolves one without naming
    /// the vault that holds it.
    ///
    /// The transition itself is the one in
    /// [`ComAmazonawsBackupCryocontrollerusermanagerState::settle_recovery_points_in`],
    /// applied to each vault's index, so a point settles identically whichever
    /// call materialises it.
    pub fn settle_recovery_points(&mut self, now: SystemTime) {
        let vault_names: Vec<String> = self.recovery_points_by_vault.keys().cloned().collect();
        for vault_name in vault_names {
            self.settle_recovery_points_in(&vault_name, now);
        }
    }
}
