// SPDX-License-Identifier: CC0-1.0

//! @op GetDICOMImportJob
//!
//! Reports the persisted import job's properties. A reporting operation (`@readonly`):
//! it writes nothing of its own, and only materialises what the clock already owed — the
//! job's move to `IN_PROGRESS`, and the terminal status and `endedAt` its completion
//! settles once the job output that completion owes has been written beneath its
//! `outputS3Uri` ([`crate::state::import_job_output`]), so a poll loop terminates and the
//! read that reports the job finished has those manifests behind it. See
//! [`crate::state::ComAmazonawsMedicalimagingAhigatewayserviceState::advance_clock`].
//!
//! ---
//!
//! # Modeled contract for `GetDICOMImportJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! Get the import job properties to learn more about the job or job progress. The jobStatus
//! refers to the execution of the import job. Therefore, an import job can return a
//! jobStatus as COMPLETED even if validation issues are discovered during the import
//! process. If a jobStatus returns as COMPLETED, we still recommend you review the output
//! manifests written to S3, as they provide details on the success or failure of individual
//! P10 object imports.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `jobId` (JobId, len 1..32, pattern ^[0-9a-z]+$) **required** — The import job
//!   identifier.
//!
//! ## Output
//!
//! * `jobProperties` (DICOMImportJobProperties) **required** — The properties of the import
//!   job.
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
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints set by the
//!   service.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetDICOMImportJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetDICOMImportJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#GetDICOMImportJob")]
#[derive(Debug)]
pub enum GetDICOMImportJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_dicom_import_job::GetDICOMImportJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetDICOMImportJobError = crate::scaffolded::Scaffolded<GetDICOMImportJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetDICOMImportJobErrorModeled> for GetDICOMImportJobError {
    fn from(modeled: GetDICOMImportJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDICOMImportJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetDICOMImportJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDICOMImportJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDICOMImportJob<Eng>
{
    type Input = GetDICOMImportJobInput;
    type Output = GetDICOMImportJobOutput;
    type Error = GetDICOMImportJobError;

    const NAME: &'static str = "GetDICOMImportJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DICOM_IMPORT_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DICOM_IMPORT_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetDICOMImportJobErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        // What the clock has already made due, before this read answers: this job's output
        // is owed at the instant its completion falls due whether or not anybody polled,
        // and until it is written the job keeps the non-terminal status it is in — so a
        // read that reports it finished has the manifests under its output prefix behind
        // it ([`crate::state::import_job_output`]). The write goes into another service,
        // so it is made outside the state lock; the status it settles is still the one the
        // clock owed.
        crate::state::import_job_output::deliver_due_outputs(
            &*self.engine,
            ctx.callback(),
            now,
            Some(&input.job_id),
        )
        .await;
        let mut store = state.lock();
        store.advance_clock(now);

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all.
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
            GetDICOMImportJobErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;
        let job = datastore.import_jobs.get(&input.job_id).ok_or_else(|| {
            GetDICOMImportJobErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Same rule, one resource kind down: the job id is not echoed either.
                message: crate::state::IMPORT_JOB_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;

        Ok(GetDICOMImportJobOutput {
            job_properties: DICOMImportJobProperties {
                job_id: job.job_id.clone(),
                job_name: job.reported_name(),
                job_status: job.job_status.as_wire().to_string(),
                datastore_id: job.datastore_id.clone(),
                data_access_role_arn: job.data_access_role_arn.clone(),
                ended_at: job.ended_at.map(crate::state::to_date_time),
                submitted_at: Some(crate::state::to_date_time(job.submitted_at)),
                input_s3_uri: job.input_s3_uri.clone(),
                output_s3_uri: job.output_s3_uri.clone(),
                message: job.reported_message(),
                import_configuration: job.import_configuration.clone(),
            },
        })
    }
}
