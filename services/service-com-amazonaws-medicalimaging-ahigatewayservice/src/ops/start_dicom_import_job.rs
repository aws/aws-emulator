// SPDX-License-Identifier: CC0-1.0

//! @op StartDICOMImportJob
//!
//! Submits a DICOM import job against a persisted data store and answers with the
//! job's identity and `SUBMITTED` status. `clientToken` is a real
//! `@idempotencyToken`: a repeat call with the same token against the same data store
//! returns the SAME `jobId` instead of submitting a second job.
//!
//! The job is returned `SUBMITTED`, the status AWS HealthImaging answers with, and the
//! clock owes its moves to `IN_PROGRESS` and then to the status its outcome settles — see
//! [`crate::state::IMPORT_JOB_IN_PROGRESS_DELAY`] and
//! [`crate::state::IMPORT_JOB_COMPLETION_DELAY`]. Nothing here sleeps.
//!
//! # What the import IMPORTS, and what it WRITES BACK
//!
//! "The import job imports DICOM P10 files … found in the S3 prefix specified by the
//! `inputS3Uri` parameter", so this call READS that prefix out of this emulator's own
//! Amazon S3 and materialises an image set for every DICOM series it found
//! ([`crate::state::read_import_prefix`], [`crate::state::imported_image_sets`]). Each
//! one comes into existence with the job, `LOCKED` / `IMPORTING` — the state and workflow
//! status the model declares for an image set a write holds — and the clock DELIVERS its
//! DICOM attributes and its image frames at the instant the job reaches its terminal
//! status, landing it `ACTIVE` / `IMPORTED`. So an attribute becomes searchable, and a
//! frame readable, exactly when the import that produced it completed; and an image set
//! exists only where a DICOM file really was read, never because a job was submitted.
//!
//! "The import job stores processing results in the file specified by the `outputS3Uri`
//! parameter", so the job also OWES that prefix its output manifests — the record of what
//! each object under the input prefix contributed — written beneath
//! `<outputS3Uri>/<datastoreId>-DicomImport-<jobId>/` when the completion falls due, and
//! before any read reports the job finished ([`crate::state::import_job_output`]).
//!
//! What the input read ESTABLISHED is what the job's outcome is decided from: an input the
//! object store says is not there, or says holds nothing, is an import with nothing to
//! import, which `FAILED` and a `message` report; an object that is not a readable DICOM
//! P10 file is one of the individual failures the manifests carry, because "an import job
//! can return a `jobStatus` as `COMPLETED` even if validation issues are discovered during
//! the import process".
//!
//! `dataAccessRoleArn` is the role the import RUNS AS, so it is resolved against the
//! account's IAM records before anything is written — the shared reading every service
//! handed a role ARN uses (`service_common::iam_role`). A role this account does not hold,
//! or one whose trust policy does not let this service assume it, is the modelled
//! `AccessDeniedException` (HTTP 403), and the refused call creates no job.
//!
//! The data store must have settled into `ACTIVE` first — the model opens this
//! operation's documentation with "Start importing bulk data into an ACTIVE data store"
//! — so a store still `CREATING`, or already `DELETING`, is refused with the modeled
//! `ConflictException` ([`crate::state::Datastore::is_active`]). That refusal governs a
//! SUBMISSION only: the `clientToken` ledger is consulted first, so a replay of a job
//! this service already accepted reports that job rather than being refused by a
//! condition that changed after the first call was answered.
//!
//! ---
//!
//! # Modeled contract for `StartDICOMImportJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Start importing bulk data into an ACTIVE data store. The import job imports DICOM P10
//! files or enhances existing DICOM files with JSON metadata. The importConfiguration
//! parameter specifies the import type. The data is found in the S3 prefix specified by the
//! inputS3Uri parameter. The import job stores processing results in the file specified by
//! the outputS3Uri parameter.
//!
//! ## Input
//!
//! * `clientToken` (ClientToken, len 1..64, pattern ^[A-Za-z0-9._-]+$, @idempotencyToken)
//!   **required** — A unique identifier for API idempotency.
//! * `dataAccessRoleArn` (RoleArn, len 20..2048, pattern
//!   ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$) **required** — The Amazon Resource Name (ARN)
//!   of the IAM role that grants permission to access medical imaging resources.
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `importConfiguration` (ImportConfiguration) — The import configuration for the import
//!   job.
//! * `inputOwnerAccountId` (AwsAccountId, len 12..12, pattern ^\d+$) — The account ID of the
//!   source S3 bucket owner.
//! * `inputS3Uri` (S3Uri, len 1..1024, pattern
//!   ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$) **required** — The input prefix path
//!   for the S3 bucket that contains the DICOM files to be imported.
//! * `jobName` (JobName, len 1..64, pattern ^[A-Za-z0-9._/#-]+$) — The import job name.
//! * `outputS3Uri` (S3Uri, len 1..1024, pattern
//!   ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$) **required** — The output prefix of the
//!   S3 bucket to upload the results of the DICOM import job.
//!
//! ## Output
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `jobId` (JobId, len 1..32, pattern ^[0-9a-z]+$) **required** — The import job
//!   identifier.
//! * `jobStatus` (JobStatus, one of: COMPLETED | FAILED | IN_PROGRESS | SUBMITTED)
//!   **required** — The import job status.
//! * `submittedAt` (Date) **required** — The timestamp when the import job was submitted.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The user does not have sufficient access to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — Updating or deleting a resource can cause an
//!   inconsistent state.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during processing of
//!   the request.
//! * `ResourceNotFoundException` (HTTP 404) — The request references a resource which does
//!   not exist.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request caused a service quota to be
//!   exceeded.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

use ::std::time::SystemTime;

use crate::state::{
    ComAmazonawsMedicalimagingAhigatewayserviceState, ImageSet, ImageSetState,
    ImageSetWorkflowStatus, ImportJob, JobStatus, PendingImageSetTransition,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartDICOMImportJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartDICOMImportJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#StartDICOMImportJob")]
#[derive(Debug)]
pub enum StartDICOMImportJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_dicom_import_job::StartDICOMImportJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartDICOMImportJobError = crate::scaffolded::Scaffolded<StartDICOMImportJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartDICOMImportJobErrorModeled> for StartDICOMImportJobError {
    fn from(modeled: StartDICOMImportJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// What the store answered about one submission, with the state lock held.
enum Admitted {
    /// The `clientToken` is already in the ledger and names a job this data store holds:
    /// the call reports that job and submits nothing.
    Replay(StartDICOMImportJobOutput),
    /// Nothing stands in the way of a fresh submission; the data store's ARN is what the
    /// image sets the import materialises are named under.
    Fresh { datastore_arn: String },
}

/// Everything the STORE decides about a submission: whether the data store is there, is
/// `ACTIVE` and has room for another job, and whether this `clientToken` has already
/// submitted one.
///
/// Called twice — once before the import's input is read and once with the write about to
/// be made — so the conditions are stated in exactly one place and a condition that
/// changed while the input was being read is answered the same way it would have been
/// answered up front.
fn admit(
    store: &mut ComAmazonawsMedicalimagingAhigatewayserviceState,
    region: &str,
    input: &StartDICOMImportJobInput,
    token_key: &str,
) -> Result<Admitted, StartDICOMImportJobError> {
    // Reached in the region this request was addressed to: a data store exists in
    // ONE region, so one held in another is not found here at all.
    let Some(datastore) = store.datastore_in(region, &input.datastore_id) else {
        return Err(StartDICOMImportJobErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            },
        )
        .into());
    };
    let datastore_is_active = datastore.is_active();
    let datastore_arn = datastore.datastore_arn.clone();
    let import_jobs = datastore.import_jobs.len();

    // `@idempotencyToken`, scoped to the data store the job runs against: a token
    // already in the ledger names the job it submitted, so the repeat call reports
    // that job's CURRENT status rather than submitting a second one.
    //
    // Resolved BEFORE this operation's refusals, exactly as the sibling idempotent
    // writer [`crate::ops::create_datastore`] resolves its own ledger: a replay
    // submits nothing, so no precondition on submitting — the store's status, the
    // per-store job cap, the role the import would run as — governs it, and a condition
    // that changed after the first call was answered cannot turn the retry into a
    // failure.
    if let Some(existing_id) = store.client_tokens.get(token_key).cloned() {
        if let Some(existing) = store
            .datastore_in(region, &input.datastore_id)
            .and_then(|datastore| datastore.import_jobs.get(&existing_id))
        {
            return Ok(Admitted::Replay(StartDICOMImportJobOutput {
                datastore_id: existing.datastore_id.clone(),
                job_id: existing.job_id.clone(),
                job_status: existing.job_status.as_wire().to_string(),
                submitted_at: crate::state::to_date_time(existing.submitted_at),
            }));
        }
    }

    // "Start importing bulk data into an ACTIVE data store": a store the clock has
    // not yet settled — still `CREATING`, or already `DELETING` — cannot take an
    // import, which is what the modelled `ConflictException` (HTTP 409) reports
    // ([`crate::state::Datastore::is_active`]). This governs a SUBMISSION, which is
    // all that reaches it now that a replay has already been answered above.
    if !datastore_is_active {
        return Err(StartDICOMImportJobErrorModeled::ConflictException(ConflictException {
            message: crate::state::DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
        })
        .into());
    }

    // Bound the memory one caller can make this emulator hold, per data store, so
    // the product of data stores and jobs stays bounded too. The cap is the
    // emulator's own ([`crate::state::MAX_IMPORT_JOBS_PER_DATASTORE`]), refused
    // with the operation's modelled quota error.
    if import_jobs >= crate::state::MAX_IMPORT_JOBS_PER_DATASTORE {
        return Err(StartDICOMImportJobErrorModeled::ServiceQuotaExceededException(
            ServiceQuotaExceededException {
                message: format!(
                    "this emulator holds at most {} import jobs per data store",
                    crate::state::MAX_IMPORT_JOBS_PER_DATASTORE
                ),
            },
        )
        .into());
    }

    Ok(Admitted::Fresh { datastore_arn })
}

/// The `StartDICOMImportJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartDICOMImportJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartDICOMImportJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartDICOMImportJob<Eng>
{
    type Input = StartDICOMImportJobInput;
    type Output = StartDICOMImportJobOutput;
    type Error = StartDICOMImportJobError;

    const NAME: &'static str = "StartDICOMImportJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_DICOM_IMPORT_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_DICOM_IMPORT_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartDICOMImportJobErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        let token_key = crate::state::client_token_key(
            &region,
            &format!("StartDICOMImportJob/{}", input.datastore_id),
            &input.client_token,
        );

        // Everything the store decides, before the import's input is read: a request the
        // store refuses reads no object and resolves no role, and a replay of a job
        // already submitted reports that job.
        {
            let mut store = state.lock();
            store.advance_clock(now);
            if let Admitted::Replay(replayed) = admit(&mut store, &region, &input, &token_key)? {
                return Ok(replayed);
            }
        }

        // "The Amazon Resource Name (ARN) of the IAM role that grants permission to access
        // medical imaging resources": an import RUNS AS that role, so it is resolved
        // against the account's IAM records before anything is written — the one reading
        // of a caller-supplied role ARN every service that takes one shares
        // (`service_common::iam_role`). A role this account does not hold, or one whose
        // trust policy does not let this service assume it, is the modelled
        // `AccessDeniedException` (HTTP 403), and the call creates nothing.
        if ::service_common::iam_role::role_is_unassumable(
            ctx,
            &region,
            &input.data_access_role_arn,
            crate::state::IMPORT_SERVICE_PRINCIPAL,
        )
        .await
        {
            return Err(StartDICOMImportJobErrorModeled::AccessDeniedException(
                AccessDeniedException {
                    message: crate::state::unassumable_role_message(&input.data_access_role_arn),
                },
            )
            .into());
        }

        // "The data is found in the S3 prefix specified by the inputS3Uri parameter":
        // what the import is asked to import is read HERE, through this emulator's own
        // Amazon S3, so the image sets the job goes on to materialise describe files that
        // really exist ([`crate::state::read_import_prefix`]). What that read ESTABLISHED
        // about the prefix is also what the job's outcome is decided from: an input the
        // object store says is not there, or says holds nothing, is an import with nothing
        // to import ([`crate::state::import_job_output::outcome_of`]).
        let mut read = crate::state::read_import_prefix(ctx, &region, &input.input_s3_uri).await;
        let instances = read.take_instances();

        let mut store = state.lock();
        store.advance_clock(now);
        // Asked again with the write about to be made: the lock was given up while the
        // input was being read, so a store that was deleted, a quota that filled or a
        // token that submitted in the meantime is answered exactly as it would have been
        // up front.
        let datastore_arn = match admit(&mut store, &region, &input, &token_key)? {
            Admitted::Replay(replayed) => return Ok(replayed),
            Admitted::Fresh { datastore_arn } => datastore_arn,
        };

        // The image sets this import will leave behind, one per DICOM series it read, with
        // their identifiers minted from the one minter this service mints every resource
        // id with.
        let imported = {
            let mut mint = || store.mint_id();
            crate::state::imported_image_sets(&instances, &mut mint)
        };

        // The outcome the job is to reach, decided from what the object store ANSWERED
        // about the input prefix rather than from how many files were readable: "an import
        // job can return a jobStatus as COMPLETED even if validation issues are discovered
        // during the import process", so an unreadable object is one of the individual
        // failures the job's own manifests carry, while an input with nothing in it at all
        // is a job that could not import.
        let failure = crate::state::import_job_output::outcome_of(&read, &input.input_s3_uri);

        // The per-data-store bounds on image sets this emulator holds, counted over what
        // the store WOULD hold once the import had landed, while the request is still
        // being checked — so no admitted import lands a data store past them. The caps are
        // the emulator's own, refused with the operation's modelled quota error, exactly
        // as the sibling image-set writer `crate::ops::copy_image_set` refuses them.
        let (held_image_sets, image_set_records) = store
            .datastore_in(&region, &input.datastore_id)
            .map_or((0, 0), |datastore| {
                (
                    datastore.held_image_sets().count(),
                    datastore.image_sets.len(),
                )
            });
        if held_image_sets + imported.len() > crate::state::MAX_IMAGE_SETS_PER_DATASTORE {
            return Err(StartDICOMImportJobErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this emulator holds at most {} image sets per data store",
                        crate::state::MAX_IMAGE_SETS_PER_DATASTORE
                    ),
                },
            )
            .into());
        }
        if image_set_records + imported.len() > crate::state::MAX_IMAGE_SET_RECORDS_PER_DATASTORE {
            return Err(StartDICOMImportJobErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this emulator holds at most {} image set records per data store",
                        crate::state::MAX_IMAGE_SET_RECORDS_PER_DATASTORE
                    ),
                },
            )
            .into());
        }

        let job_id = store.mint_id();
        let completed_due_at = now + crate::state::IMPORT_JOB_COMPLETION_DELAY;
        // "The import job stores processing results in the file specified by the
        // outputS3Uri parameter": the manifests of what every object under the input
        // prefix contributed, rendered from the read that established it and owed to the
        // output prefix from here on. A job whose input nothing answered about owes none —
        // nothing was established about it, so there is nothing to report — which is also
        // why it is the only job whose terminal status the clock settles on its own.
        let output_owed = read
            .was_answered()
            .then(|| {
                crate::state::import_job_output::output_location(
                    &input.output_s3_uri,
                    &input.datastore_id,
                    &job_id,
                )
                .map(|(bucket, key_prefix)| crate::state::import_job_output::JobOutput {
                    state: crate::state::import_job_output::DeliveryState::Owed,
                    region: region.clone(),
                    bucket,
                    key_prefix,
                    objects: crate::state::import_job_output::job_output_objects(
                        &input.datastore_id,
                        &job_id,
                        &read,
                        &imported,
                        failure.as_deref(),
                    ),
                })
            })
            .flatten();
        let owes_output = output_owed.is_some();
        let job = ImportJob {
            job_id: job_id.clone(),
            job_name: input.job_name.clone(),
            job_status: JobStatus::Submitted,
            datastore_id: input.datastore_id.clone(),
            data_access_role_arn: input.data_access_role_arn.clone(),
            input_s3_uri: input.input_s3_uri.clone(),
            output_s3_uri: input.output_s3_uri.clone(),
            input_owner_account_id: input.input_owner_account_id.clone(),
            import_configuration: input.import_configuration.clone(),
            submitted_at: now,
            ended_at: None,
            failure,
            output_owed,
            in_progress_due_at: now + crate::state::IMPORT_JOB_IN_PROGRESS_DELAY,
            completed_due_at,
        };
        let job_status = job.job_status.as_wire().to_string();
        let submitted_at = crate::state::to_date_time(job.submitted_at);

        // Persist FIRST, and refuse rather than branch past the insert: answering 200
        // with a jobId that was never stored would dress absence as success.
        let datastore = store
            .datastore_in_mut(&region, &input.datastore_id)
            .ok_or_else(|| {
                StartDICOMImportJobErrorModeled::InternalServerException(InternalServerException {
                    message: "the data store could not be reached while submitting the import job"
                        .to_string(),
                })
            })?;
        datastore.import_jobs.insert(job_id.clone(), job);

        // Each image set the import read comes into existence with the job: `IMPORTING` is
        // the workflow status the model declares for one an import is writing, and
        // `LOCKED` is the state an image set a write holds is in — so nothing can be read
        // out of it or written into it while the import runs. Its DICOM attributes and its
        // image frames are DELIVERED by the clock at the instant the job reaches its
        // terminal status, which is when the image set lands `ACTIVE` / `IMPORTED`: an
        // attribute is searchable, and a frame readable, exactly when the import that
        // delivered it completed and not before
        // (`crate::state::ImageSet::advance_to`).
        for one in imported {
            let mut image_set = ImageSet::new(
                one.image_set_id.clone(),
                crate::state::image_set_arn(&datastore_arn, &one.image_set_id),
                ImageSetState::Locked,
                ImageSetWorkflowStatus::Importing,
                // An imported image set is in the primary DICOM hierarchy: it is the
                // original of the study's content rather than a copy taken out of it.
                true,
                false,
                now,
            );
            image_set.pending = Some(PendingImageSetTransition {
                due_at: completed_due_at,
                image_set_state: ImageSetState::Active,
                workflow_status: ImageSetWorkflowStatus::Imported,
                delivered_image_frames: Some(one.image_frames),
                delivered_attributes: Some(one.attributes),
                // An import does not retire an image set: it brings one into existence.
                retires_image_set: false,
            });
            datastore
                .image_sets
                .insert(one.image_set_id.clone(), image_set);
        }
        store.remember_client_token(token_key, job_id.clone());
        // Given up before the delivery below is armed: arming takes the same lock.
        drop(store);

        // A client that submits a job and then watches the output prefix never calls this
        // service again, so the job's output is armed for the instant its completion falls
        // due rather than left to a read that may never come. It claims the same delivery
        // a read claims, so whichever gets there first makes it exactly once. A job that
        // owes no output has nothing to arm: the clock settles it on its own.
        if owes_output {
            crate::state::import_job_output::deliver_when_due(
                ::std::sync::Arc::clone(&self.engine),
                ctx.callback().clone(),
                input.datastore_id.clone(),
                job_id.clone(),
                completed_due_at,
            );
        }

        Ok(StartDICOMImportJobOutput {
            datastore_id: input.datastore_id,
            job_id,
            job_status,
            submitted_at,
        })
    }
}
