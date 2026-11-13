// SPDX-License-Identifier: CC0-1.0

//! @op ListDICOMImportJobs
//!
//! Enumerates the import jobs ONE data store owns, oldest first, optionally filtered by
//! `jobStatus`, one `maxResults`-sized page at a time. A reporting operation
//! (`@readonly`): it writes nothing of its own, and only materialises the status changes
//! the clock already owed — a job's move to `IN_PROGRESS` and then to `COMPLETED` with
//! its `endedAt` — exactly as `GetDICOMImportJob` does, so the listing and a read of one
//! job never disagree.
//!
//! A data store this emulator does not hold is refused with `ResourceNotFoundException`
//! (404) rather than answered with an empty list: the jobs are the data store's, so
//! "none" and "no such data store" are different answers.
//!
//! The continuation token is opaque on the wire — hex over a marked encoding of the FULL
//! sort key the next page starts at, through the service's one codec
//! ([`crate::state::encode_page_token`]) — so a string this service did not mint is
//! refused, the marker carries the data store AND the `jobStatus` filter the position
//! belongs to (a token minted against another data store, or under another status filter,
//! is not a position in the walk this request makes), and paging cannot skip or repeat a
//! job even when the one the token named is gone.
//!
//! ---
//!
//! # Modeled contract for `ListDICOMImportJobs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-medicalimaging-ahigatewayservice/smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json")
//! ```
//!
//! List import jobs created for a specific data store.
//!
//! ## Input
//!
//! * `datastoreId` (DatastoreId, pattern ^[0-9a-z]{32}$) **required** — The data store
//!   identifier.
//! * `jobStatus` (JobStatus, one of: COMPLETED | FAILED | IN_PROGRESS | SUBMITTED) — The
//!   filters for listing import jobs based on status.
//! * `maxResults` (Integer) — The max results count. The upper bound is determined by load
//!   testing.
//! * `nextToken` (NextToken, len 1..8192, pattern ^\p{ASCII}{0,8192}$) — The pagination token
//!   used to request the list of import jobs on the next page.
//!
//! ## Output
//!
//! * `jobSummaries` (DICOMImportJobSummaries) **required** — A list of job summaries.
//! * `nextToken` (NextToken, len 1..8192, pattern ^\p{ASCII}{0,8192}$) — The pagination token
//!   used to retrieve the list of import jobs on the next page.
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

use crate::state::JobStatus;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListDICOMImportJobsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-medicalimaging-ahigatewayservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListDICOMImportJobsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.medicalimaging#ListDICOMImportJobs")]
#[derive(Debug)]
pub enum ListDICOMImportJobsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_dicom_import_jobs::ListDICOMImportJobsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListDICOMImportJobsError = crate::scaffolded::Scaffolded<ListDICOMImportJobsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListDICOMImportJobsErrorModeled> for ListDICOMImportJobsError {
    fn from(modeled: ListDICOMImportJobsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListDICOMImportJobs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListDICOMImportJobs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListDICOMImportJobs<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListDICOMImportJobs<Eng>
{
    type Input = ListDICOMImportJobsInput;
    type Output = ListDICOMImportJobsOutput;
    type Error = ListDICOMImportJobsError;

    const NAME: &'static str = "ListDICOMImportJobs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_DICOM_IMPORT_JOBS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_DICOM_IMPORT_JOBS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx);
        let now = ::std::time::SystemTime::now();

        // The status filter must name a modelled `JobStatus`. The boundary validator
        // already refused anything else, so an unparseable value here can only come from
        // a caller bypassing it.
        let status_filter = match input.job_status.as_deref() {
            None => None,
            Some(value) => match JobStatus::from_wire(value) {
                Some(status) => Some(status),
                None => {
                    return Err(ListDICOMImportJobsErrorModeled::ValidationException(
                        ValidationException {
                            // Worded through the one refusal formatter this service
                            // owns, so a refusal raised here reads exactly like one the
                            // boundary validator stated.
                            message: crate::state::one_validation_error(
                                &smithy_server_runtime::Violation {
                                    path: "jobStatus".to_string(),
                                    kind: smithy_server_runtime::ViolationKind::EnumValue(
                                        JOB_STATUS_VALUES
                                            .iter()
                                            .map(|value| (*value).to_string())
                                            .collect(),
                                    ),
                                    value: Some(value.to_string()),
                                    sensitive: false,
                                },
                            ),
                        },
                    )
                    .into());
                }
            },
        };

        // The token is this service's own opaque handle, marked with the data store AND
        // the status filter the position belongs to; only one this service minted for
        // THAT collection can be honoured, and anything else is a client error rather
        // than an empty page. The refusal is the service's FIXED sentence: it does not
        // vary with the token, so it reveals nothing about how a real one is built.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(
                decode_page_token(&input.datastore_id, status_filter, token).ok_or_else(|| {
                    ListDICOMImportJobsErrorModeled::ValidationException(ValidationException {
                        message: crate::state::INVALID_NEXT_TOKEN_MESSAGE.to_string(),
                    })
                })?,
            ),
        };

        // The page-size rule is the service's one ([`crate::state::list_page_size`]), so
        // every listing bounds a page the same way.
        let page_size = crate::state::list_page_size(input.max_results);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListDICOMImportJobsErrorModeled::InternalServerException(InternalServerException {
                message: "the medical-imaging state store is unavailable".to_string(),
            })
        })?;
        // What the clock has already made due, before this listing answers: a job's output
        // is owed at the instant its completion falls due whether or not anybody polled,
        // and until it is written the job keeps the non-terminal status it is in
        // ([`crate::state::import_job_output`]). This is the same materialisation the
        // status move itself is, made where it can be: outside the state lock, because it
        // writes into another service.
        crate::state::import_job_output::deliver_due_outputs(
            &*self.engine,
            ctx.callback(),
            now,
            None,
        )
        .await;
        let mut store = state.lock();
        store.advance_clock(now);

        // Reached in the region this request was addressed to: a data store exists in
        // ONE region, so one held in another is not found here at all.
        let datastore = store.datastore_in(&region, &input.datastore_id).ok_or_else(|| {
            ListDICOMImportJobsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                // Constant, and stated by resource KIND: the requested id is not
                // reflected back (`crate::state::DATASTORE_NOT_FOUND_MESSAGE`).
                message: crate::state::DATASTORE_NOT_FOUND_MESSAGE.to_string(),
            })
        })?;

        // Total order over the whole collection — submission instant first, job id to
        // break a tie — so a page boundary is stable no matter how the underlying map
        // iterates.
        let mut ordered: Vec<&crate::state::ImportJob> = datastore
            .import_jobs
            .values()
            .filter(|job| status_filter.is_none_or(|status| job.job_status == status))
            .collect();
        ordered.sort_by(|left, right| sort_key(left).cmp(&sort_key(right)));

        // Resume at the first job whose FULL sort key is at or after the token's.
        // Comparing the whole key — not the id alone — is what makes the walk correct
        // when the job the token named is no longer there: it can neither land before a
        // page already served (repeating a job) nor past one never seen (skipping it).
        let start = match &resume_from {
            None => 0,
            Some(cursor) => ordered
                .iter()
                .position(|job| sort_key(job) >= cursor.as_key())
                .unwrap_or(ordered.len()),
        };

        let end = start.saturating_add(page_size).min(ordered.len());
        let job_summaries: Vec<DICOMImportJobSummary> = ordered[start..end]
            .iter()
            .map(|job| DICOMImportJobSummary {
                job_id: job.job_id.clone(),
                // `DICOMImportJobSummary.jobName` is @required while the submit request
                // leaves it optional, so a nameless job reports its own id — the same
                // fallback `GetDICOMImportJob` reports.
                job_name: job.reported_name(),
                job_status: job.job_status.as_wire().to_string(),
                datastore_id: job.datastore_id.clone(),
                data_access_role_arn: Some(job.data_access_role_arn.clone()),
                ended_at: job.ended_at.map(crate::state::to_date_time),
                submitted_at: Some(crate::state::to_date_time(job.submitted_at)),
                message: job.reported_message(),
            })
            .collect();

        let next_token = ordered
            .get(end)
            .map(|job| encode_page_token(&input.datastore_id, status_filter, job));

        Ok(ListDICOMImportJobsOutput {
            job_summaries,
            next_token,
        })
    }
}

/// The marker every token this listing mints carries: this operation's own prefix, the
/// data store the position belongs to, and the `jobStatus` the walk it is a position in
/// was narrowed by ([`crate::state::page_token_filter_segment`]).
///
/// A hex string this service never issued, one minted for another collection, one minted
/// against another data store and one minted under another status filter are all refused
/// rather than honoured as a resume point — the import jobs of two data stores are two
/// collections, and the filter decides which of a data store's jobs are in the walk at
/// all, so a position in the jobs one status selects is not a position in the jobs another
/// selects nor in the unfiltered whole. The codec itself is the service's one
/// ([`crate::state::encode_page_token`]), so every listing's token is built one way.
fn page_token_marker(datastore_id: &str, status_filter: Option<JobStatus>) -> String {
    format!(
        "ahi-import-jobs-v1:{datastore_id}:{}:",
        crate::state::page_token_filter_segment(status_filter.map(JobStatus::as_wire))
    )
}

/// The total order the listing walks: submission instant first, job id to break a tie —
/// so a page boundary is stable no matter how the underlying map iterates.
fn sort_key(job: &crate::state::ImportJob) -> (u64, u32, &str) {
    crate::state::page_sort_key(job.submitted_at, &job.job_id)
}

/// Encode the sort key the next page starts at as this listing's opaque `nextToken`.
fn encode_page_token(
    datastore_id: &str,
    status_filter: Option<JobStatus>,
    first_of_next_page: &crate::state::ImportJob,
) -> String {
    crate::state::encode_page_token(
        &page_token_marker(datastore_id, status_filter),
        first_of_next_page.submitted_at,
        &first_of_next_page.job_id,
    )
}

/// Decode a token produced by [`encode_page_token`] against `datastore_id` and
/// `status_filter`, or `None` when it was not one.
fn decode_page_token(
    datastore_id: &str,
    status_filter: Option<JobStatus>,
    token: &str,
) -> Option<crate::state::PageCursor> {
    crate::state::decode_page_token(&page_token_marker(datastore_id, status_filter), token)
}
