// SPDX-License-Identifier: CC0-1.0

//! @op StartRestoreJob
//!
//! Starts a restore FROM a stored recovery point: the point the
//! `RecoveryPointArn` names is resolved against [`crate::state`]
//! ([`crate::state::ComAmazonawsBackupCryocontrollerusermanagerState::restorable_recovery_point`])
//! before anything is persisted, and the job this call stores
//! ([`crate::state::RestoreJobRecord`]) is the one record every call that names
//! its `RestoreJobId` answers from.
//!
//! The two refusal CLASSES this operation and its sibling reads keep apart:
//!
//! * the recovery point is an INPUT to the restore, so a `RecoveryPointArn`
//!   that resolves to no stored recovery point — fabricated, belonging to a
//!   backup that has not finished, or already deleted — is refused as the value
//!   of that member ([`crate::state::recovery_point_not_found_value`]), and a
//!   value that is not an ARN at all is refused the same class
//!   ([`crate::state::rejected_arn`]); nothing is stored either way;
//!   not-yet-finished and already-gone are one condition, as they are for every
//!   call that resolves a stored point;
//! * the restore job is a RESOURCE, so a `RestoreJobId` naming none is reported
//!   absent by the reads ([`crate::state::restore_job_not_found`]) — a client
//!   can therefore tell a bad recovery-point reference from a bad restore-job
//!   reference.
//!
//! The `Metadata` is read the same way as the recovery point: it is an INPUT to
//! the restore, so metadata that carries no value for a key the resource type's
//! restore requires is refused as that member's value
//! ([`crate::state::rejected_restore_metadata`]) and nothing is stored — the
//! restore of a type whose target the metadata does not name has nothing to
//! create, and a start the service accepts is one whose restore has a target.
//!
//! Ahead of both stands the authorization decision on the point the request
//! names ([`crate::state::authorized_recovery_point`]): an ARN of another
//! account is refused `AccessDeniedException` (HTTP 403) before the store is
//! read, since no resolution of this account's state could reach it.
//!
//! A started job is reported in the non-terminal status AWS answers with and
//! the clock owes it the terminal one
//! ([`crate::state::RESTORE_JOB_SETTLE_AFTER`]). The restore ITSELF is
//! performed by this call: the protected resource is re-created in the service
//! that owns it ([`crate::state::perform_restore`]), and the job reports as
//! `CreatedResourceArn` the ARN that service answered with — so a resource no
//! service holds is never reported as created, and a restore of a type this
//! emulator cannot perform ends in the modelled failure status carrying the
//! reason. Every status the job MOVES INTO after this call is published onto the
//! account's default EventBridge bus as a `Restore Job State Change`
//! ([`crate::state::RESTORE_JOB_STATE_CHANGE`]), the same way a backup job's and
//! a copy job's moves are, so a client watching the bus needs to make no further
//! call.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `StartRestoreJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Recovers the saved resource identified by an Amazon Resource Name (ARN).
//!
//! ## Input
//!
//! * `CopySourceTagsToRestoredResource` (Boolean2, default false) **required** — This is an
//!   optional parameter. If this equals True, tags included in the backup will be copied to
//!   the restored resource. This can only be applied to backups created through Backup.
//! * `IamRoleArn` (IAMRoleArn) — The Amazon Resource Name (ARN) of the IAM role that Backup
//!   uses to create the target resource; for example:
//!   arn:aws:iam::123456789012:role/S3Access.
//! * `IdempotencyToken` (string, @idempotencyToken) — A customer-chosen string that you can
//!   use to distinguish between otherwise identical calls to StartRestoreJob. Retrying a
//!   successful request with the same idempotency token results in a success message with no
//!   action taken.
//! * `Metadata` (Metadata) **required** — A set of metadata key-value pairs. You can get
//!   configuration metadata about a resource at the time it was backed up by calling
//!   GetRecoveryPointRestoreMetadata. However, values in addition to those provided by
//!   GetRecoveryPointRestoreMetadata might be required to restore a resource. For example,
//!   you might need to provide a new resource name if the original already exists. For more
//!   information about the metadata for each resource, see the following: Metadata for Amazon
//!   Aurora Metadata for Amazon DocumentDB Metadata for CloudFormation Metadata for Amazon
//!   DynamoDB Metadata for Amazon EBS Metadata for Amazon EC2 Metadata for Amazon EFS
//!   Metadata for Amazon EKS Metadata for Amazon FSx Metadata for …
//! * `RecoveryPointArn` (ARN) **required** — An ARN that uniquely identifies a recovery
//!   point; for example,
//!   arn:aws:backup:us-east-1:123456789012:recovery-point:1EB3B5E7-9EB0-435A-A80B-108B488B0D45.
//! * `ResourceType` (ResourceType, pattern ^[a-zA-Z0-9\-\_\.]{1,50}$) — Starts a job to
//!   restore a recovery point for one of the following resources: Aurora - Amazon Aurora
//!   DocumentDB - Amazon DocumentDB CloudFormation - CloudFormation DynamoDB - Amazon
//!   DynamoDB EBS - Amazon Elastic Block Store EC2 - Amazon Elastic Compute Cloud EFS -
//!   Amazon Elastic File System EKS - Amazon Elastic Kubernetes Service FSx - Amazon FSx
//!   Neptune - Amazon Neptune RDS - Amazon Relational Database Service Redshift - Amazon
//!   Redshift Storage Gateway - Storage Gateway S3 - Amazon Simple Storage Service Timestream
//!   - Amazon Timestream VirtualMachine - Virtual machines
//!
//! ## Output
//!
//! * `RestoreJobId` (RestoreJobId) — Uniquely identifies the job that restores a recovery
//!   point.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `InvalidRequestException` (HTTP 400) — Indicates that something is wrong with the input
//!   to the request. For example, a parameter is of the wrong type.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartRestoreJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartRestoreJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#StartRestoreJob")]
#[derive(Debug)]
pub enum StartRestoreJobErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    InvalidRequestException(InvalidRequestException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_restore_job::StartRestoreJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartRestoreJobError = crate::scaffolded::Scaffolded<StartRestoreJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartRestoreJobErrorModeled> for StartRestoreJobError {
    fn from(modeled: StartRestoreJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartRestoreJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartRestoreJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartRestoreJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartRestoreJob<Eng>
{
    type Input = StartRestoreJobInput;
    type Output = StartRestoreJobOutput;
    type Error = StartRestoreJobError;

    const NAME: &'static str = "StartRestoreJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_RESTORE_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_RESTORE_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);

        // The model types `RecoveryPointArn` as `ARN` with no pattern, so the
        // framework cannot refuse a malformed one; AWS does, rather than
        // treating a value that could never name a recovery point as one it
        // merely failed to find. ONE check and one sentence for every such
        // member in this service.
        if let Some(refusal) =
            crate::state::rejected_arn("RecoveryPointArn", &input.recovery_point_arn)
        {
            return Err(StartRestoreJobErrorModeled::InvalidParameterValueException(refusal).into());
        }
        // The same reading for the role the restore would run as, which the
        // model also types as an unpatterned ARN.
        if let Some(iam_role_arn) = input.iam_role_arn.as_deref() {
            if let Some(refusal) = crate::state::rejected_arn("IamRoleArn", iam_role_arn) {
                return Err(
                    StartRestoreJobErrorModeled::InvalidParameterValueException(refusal).into(),
                );
            }
        }

        // The authorization decision on the recovery point this restore reads,
        // taken BEFORE it is resolved: an ARN of another account is refused for
        // lack of privileges rather than reported as a value that names no
        // stored point (`crate::state::authorized_recovery_point`).
        crate::state::authorized_recovery_point(&input.recovery_point_arn)?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartRestoreJobErrorModeled::ServiceUnavailableException(ServiceUnavailableException {
                message: Some("the backup vault store is unavailable".to_string()),
                ..Default::default()
            })
        })?;

        // What the restore has to create, read out of the store under the lock
        // and used to perform it once the lock is released. A refusal is carried
        // OUT of the block rather than returned from inside it, so the moves the
        // settles below materialised are published either way.
        let started: Result<Started, StartRestoreJobError> = {
            let mut store = state.lock();

            // The transitions the clock already owes: a recovery point whose
            // backup came due completed at that instant whether or not anybody
            // called, and a completed point is one this call can restore from;
            // the stored restore jobs settle for the same reason, so a replayed
            // token answers from the job's current status.
            store.settle_recovery_points(now);
            store.settle_restore_jobs(now);
            // Whatever those transitions moved owes the bus an event, whichever
            // call materialised it (`crate::state::events`).
            store.record_job_state_changes();

            // A repeated `IdempotencyToken` takes no second action: it answers
            // with the job that token started, and performs no second restore.
            // Resolved BEFORE the recovery point, since a replay necessarily
            // repeats the reference the first call made.
            if let Some(existing) = input
                .idempotency_token
                .as_deref()
                .and_then(|token| store.restore_job_by_idempotency_token(token))
            {
                Ok(Started::Replayed(existing.restore_job_id.clone()))
            } else {
                Self::stored_restore(&mut store, &input, now)
            }
        };

        // Nothing was published while the lock was held; this is where the moves
        // recorded above reach the bus, before this call's own answer is decided.
        crate::state::publish_job_state_changes(ctx, &*self.engine).await;

        let restore_job_id = match started? {
            // A replay performs nothing: the restore its token started is the
            // only one, and the job it stored is what this call answers with.
            Started::Replayed(restore_job_id) => restore_job_id,
            Started::Stored {
                restore_job_id,
                resource_type,
                source_resource_arn,
                metadata,
                source_tags,
            } => {
                // THE RESTORE ITSELF: the resource is created in the service that
                // owns it, over the emulator's own in-process transport, with no
                // state lock held across the hop. What comes back is that service's
                // own answer — the ARN of the resource it now holds, or why it holds
                // none — and it is recorded on the stored job, so every read of that
                // job reports what the restore came to.
                //
                // Started as its OWN task, the way this service's clock pump is
                // started: `StartRestoreJob` "recovers the saved resource" without
                // waiting for the recovery to finish, so the call answers with the
                // job's id and the restore runs on. With no async runtime to spawn
                // onto there is none to run on, and the restore is performed before
                // this call answers instead.
                let restoring = Restoring {
                    state: ::engine::ServiceState::clone(&state),
                    restore_job_id: restore_job_id.clone(),
                    callback: ctx.callback().clone(),
                    region,
                    resource_type,
                    source_resource_arn,
                    metadata,
                    source_tags,
                    copy_source_tags: input.copy_source_tags_to_restored_resource,
                };
                match ::tokio::runtime::Handle::try_current() {
                    Ok(runtime) => {
                        runtime.spawn(restoring.perform());
                    }
                    Err(_) => restoring.perform().await,
                }
                restore_job_id
            }
        };

        // The transition this job is owed falls due on the clock, not on the next
        // call: the pump publishes it when it does, so a client watching the bus
        // needs to make no further call (`crate::state::events`).
        crate::state::drive_job_clock(ctx, &self.engine);
        Ok(StartRestoreJobOutput {
            restore_job_id: Some(restore_job_id),
        })
    }
}

/// What a start resolved to under the store's lock: the job a repeated
/// `IdempotencyToken` replays, or the job this call stored and what performing
/// its restore needs.
enum Started {
    Replayed(String),
    Stored {
        restore_job_id: String,
        resource_type: Option<String>,
        source_resource_arn: String,
        metadata: ::std::collections::BTreeMap<String, String>,
        source_tags: ::std::collections::BTreeMap<String, String>,
    },
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> StartRestoreJob<Eng> {
    /// Resolve the recovery point this restore reads and store the job it starts.
    ///
    /// Separate from `handle` so this store's lock lives for exactly this step:
    /// the publication `handle` makes afterwards awaits, and a guard must not be
    /// held across an await.
    fn stored_restore(
        store: &mut crate::state::ComAmazonawsBackupCryocontrollerusermanagerState,
        input: &StartRestoreJobInput,
        now: ::std::time::SystemTime,
    ) -> Result<Started, StartRestoreJobError> {
        // The recovery point is an INPUT to the restore, so one that
        // resolves to nothing stored is refused as that member's value and
        // no job is persisted. Resolved before anything is written.
        let Some(point) = store.restorable_recovery_point(&input.recovery_point_arn) else {
            return Err(StartRestoreJobErrorModeled::InvalidParameterValueException(
                crate::state::recovery_point_not_found_value(),
            )
            .into());
        };
        // Taken off the resolved point, so the job can report nothing about
        // a recovery point the store does not hold.
        let recovery_point_arn = point.arn.clone();
        let source_resource_arn = point.resource_arn.clone();
        let backup_vault_arn = point.backup_vault_arn.clone();
        let recovery_point_creation_date = point.creation_date;
        // The tags "included in the backup", which the restore copies to
        // the resource it creates when the request asks it to.
        let source_tags = point.tags.clone();
        // "Starts a job to restore a recovery point for one of the
        // following resources" — the request may name the type, and when it
        // does not the type of the resource the point was taken of is what
        // the restore is of.
        let resource_type = input
            .resource_type
            .clone()
            .or_else(|| point.resource_type.clone());

        // What the restore of THAT type cannot be performed without: metadata
        // that carries no value for a key the type requires is refused as that
        // member's value, before anything is written, so a start the service
        // accepts is one whose restore has a target
        // (`crate::state::restore_metadata`).
        let metadata: ::std::collections::BTreeMap<String, String> =
            input.metadata.clone().into_iter().collect();
        if let Some(refusal) = crate::state::rejected_restore_metadata(
            resource_type.as_deref(),
            &recovery_point_arn,
            &metadata,
        ) {
            return Err(
                StartRestoreJobErrorModeled::InvalidParameterValueException(refusal).into(),
            );
        }

        // The store is in memory and every request can add to it, so the
        // number of restore jobs it keeps is bounded; AWS refuses a restore
        // it will not start with the same modelled class.
        if store.restore_jobs.len() >= crate::state::MAX_RESTORE_JOBS {
            return Err(StartRestoreJobErrorModeled::InvalidRequestException(
                InvalidRequestException {
                    message: Some(format!(
                        "this emulator keeps at most {} restore jobs",
                        crate::state::MAX_RESTORE_JOBS
                    )),
                    ..Default::default()
                },
            )
            .into());
        }

        let restore_job_id = store.next_id(now);
        let record = crate::state::RestoreJobRecord {
            restore_job_id: restore_job_id.clone(),
            recovery_point_arn,
            source_resource_arn: source_resource_arn.clone(),
            backup_vault_arn,
            recovery_point_creation_date,
            iam_role_arn: input.iam_role_arn.clone(),
            resource_type: resource_type.clone(),
            // The restore is not done the instant this call answers, so the
            // job starts in the non-terminal status AWS reports for one and
            // the clock owes it the terminal one.
            status: crate::state::RESTORE_JOB_STATUS_PENDING.to_string(),
            status_message: None,
            creation_date: now,
            settles_at: now
                .checked_add(crate::state::RESTORE_JOB_SETTLE_AFTER)
                .unwrap_or(now),
            completion_date: None,
            metadata: metadata.clone(),
            // No validation result has been recorded for this restore yet:
            // `PutRestoreValidationResult` is what records one.
            validation_status: None,
            validation_status_message: None,
            created_resource_arn: None,
            copy_source_tags_to_restored_resource: input.copy_source_tags_to_restored_resource,
            idempotency_token: input.idempotency_token.clone(),
        };
        let metadata = record.metadata.clone();
        // The job is stored BEFORE the restore is performed, so the id this
        // call answers with addresses a job every read can already resolve
        // and a repeat of the token cannot start a second restore.
        store.restore_jobs.insert(restore_job_id.clone(), record);
        // The status this call stored is what this call's own response reports, so
        // recording it announces nothing: it is the status the job's LATER moves
        // are published against (`crate::state::events`).
        store.record_job_state_changes();
        Ok(Started::Stored {
            restore_job_id,
            resource_type,
            source_resource_arn,
            metadata,
            source_tags,
        })
    }
}

/// One restore waiting to be performed: everything it needs, owned, so it can
/// run as a task of its own rather than inside the request that asked for it.
struct Restoring {
    state: ::engine::ServiceState<crate::state::ComAmazonawsBackupCryocontrollerusermanagerState>,
    restore_job_id: String,
    callback: smithy_server_runtime::EmbeddedEmulator,
    region: String,
    resource_type: Option<String>,
    source_resource_arn: String,
    metadata: ::std::collections::BTreeMap<String, String>,
    source_tags: ::std::collections::BTreeMap<String, String>,
    copy_source_tags: bool,
}

impl Restoring {
    /// Perform the restore and record on the stored job what it came to.
    ///
    /// The resource the restore created is recorded as the ARN the owning
    /// service answered with; a restore that created nothing moves the job to
    /// the modelled failure status with the reason, which is terminal, so the
    /// clock never reports it `COMPLETED` afterwards.
    ///
    /// BOUNDED by construction: it makes at most the two in-process calls the
    /// restore of one resource needs ([`crate::state::perform_restore`]), with
    /// retries and timeouts disabled, and then returns — there is no loop, no
    /// wait and no work sized by the request, and one task is started per
    /// stored restore job, which [`crate::state::MAX_RESTORE_JOBS`] caps.
    async fn perform(self) {
        let outcome = crate::state::perform_restore(
            &self.callback,
            &self.region,
            self.resource_type.as_deref(),
            &self.source_resource_arn,
            &self.metadata,
            &self.source_tags,
            self.copy_source_tags,
        )
        .await;
        let now = ::std::time::SystemTime::now();
        {
            let mut store = self.state.lock();
            let Ok(job) = store.stored_restore_job_mut(&self.restore_job_id) else {
                return;
            };
            match outcome {
                crate::state::RestoreOutcome::Created(created_resource_arn) => {
                    job.created_resource_arn = Some(created_resource_arn);
                }
                crate::state::RestoreOutcome::Failed(reason) => {
                    job.status = crate::state::RESTORE_JOB_STATUS_FAILED.to_string();
                    job.status_message = Some(reason);
                    job.completion_date = Some(now);
                }
            }
            // A restore that created nothing moved the job to its terminal status
            // HERE rather than on the clock, so this is the call that owes the bus
            // that move; a restore that created the resource moved nothing yet, and
            // records none (`crate::state::events`).
            store.record_job_state_changes();
        }
        // With the lock released, so none is held across the publication's await.
        crate::state::publish_queued(&self.callback, &self.state).await;
        // The resource recorded above is what makes this job's terminal status due on
        // the clock, so this is where the pump that waits for it is started.
        crate::state::drive_clock(&self.callback, &self.state);
    }
}
