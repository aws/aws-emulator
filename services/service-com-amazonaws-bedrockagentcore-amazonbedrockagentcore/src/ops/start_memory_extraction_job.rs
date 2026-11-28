// SPDX-License-Identifier: CC0-1.0

//! @op StartMemoryExtractionJob
//! Starts one unit of long-term memory extraction work a memory container holds: the
//! messages of the session the `jobId` names that no earlier attempt picked up.
//!
//! The rule: the job the request names is picked up from what the container has accepted
//! for extraction — the same units `ListMemoryExtractionJobs` reports — and the start
//! records the WATERMARK of the records it covered, so the job is no longer eligible to be
//! started while nothing new has been accepted into its session, and a message stored
//! afterwards composes that session's next unit of work ("when earlier extraction attempts
//! have left events unprocessed, this job will pick up and extract those as well"). The
//! answer carries the `jobId` the request asked to start.
//!
//! A `jobId` naming neither a unit of work the container holds nor a job a start of this
//! container already picked up is a `ResourceNotFoundException`; the operation is
//! `@idempotent`, so repeating a start of the SAME job answers with that job rather than
//! refusing it or starting a second one, which is also what makes the `@idempotencyToken`
//! nothing to record: the answer is the `jobId` the request itself named.
//!
//! What a start does NOT do here is produce memory records. The structured records an
//! extraction yields are a model's reading of a conversation under the memory strategies
//! the CONTROL plane holds, which this data plane can neither read nor run, so this
//! operation picks the messages up and reports no record it did not produce — the same
//! position `IngestData` takes for the content it stores.
//!
//! The pick-up itself lives beside the store it reads and writes
//! ([`crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::start_extraction_job`]),
//! so the `jobId` this operation starts and the one the listing reports have one home.
//!
//! ---
//!
//! # Modeled contract for `StartMemoryExtractionJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json")
//! ```
//!
//! Starts a memory extraction job that processes events that failed extraction previously
//! in an AgentCore Memory resource and produces structured memory records. When earlier
//! extraction attempts have left events unprocessed, this job will pick up and extract
//! those as well. To use this operation, you must have the
//! bedrock-agentcore:StartMemoryExtractionJob permission.
//!
//! ## Input
//!
//! * `clientToken` (String, @idempotencyToken) — A unique, case-sensitive identifier to
//!   ensure idempotent processing of the request.
//! * `extractionJob` (ExtractionJob) **required** — Extraction job to start in this
//!   operation.
//! * `memoryId` (MemoryId, len 12.., pattern
//!   ^(arn:(aws|aws-cn|aws-us-gov):bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10
//!   …) **required** — The unique identifier of the memory for which to start extraction
//!   jobs.
//!
//! ## Output
//!
//! * `jobId` (String) **required** — Extraction Job ID that was attempted to start.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The exception that occurs when you do not have
//!   sufficient permissions to perform an action. Verify that your IAM policy includes the
//!   necessary permissions for the operation you are trying to perform.
//! * `ResourceNotFoundException` (HTTP 404) — The exception that occurs when the specified
//!   resource does not exist. This can happen when using an invalid identifier or when trying
//!   to access a resource that has been deleted.
//! * `ServiceException` (HTTP 500) — The service encountered an internal error. Try your
//!   request again later.
//! * `ServiceQuotaExceededException` (HTTP 402) — The exception that occurs when the request
//!   would cause a service quota to be exceeded. Review your service quotas and either reduce
//!   your request rate or request a quota increase.
//! * `ThrottledException` (HTTP 429) — The request was denied due to request throttling.
//!   Reduce the frequency of requests and try again.
//! * `ValidationException` (HTTP 400) — The exception that occurs when the input fails to
//!   satisfy the constraints specified by the service. Check the error message for details
//!   about which input parameter is invalid and correct your request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartMemoryExtractionJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartMemoryExtractionJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcore#StartMemoryExtractionJob")]
#[derive(Debug)]
pub enum StartMemoryExtractionJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceException(ServiceException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottledException(ThrottledException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_memory_extraction_job::StartMemoryExtractionJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartMemoryExtractionJobError = crate::scaffolded::Scaffolded<StartMemoryExtractionJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartMemoryExtractionJobErrorModeled> for StartMemoryExtractionJobError {
    fn from(modeled: StartMemoryExtractionJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartMemoryExtractionJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartMemoryExtractionJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartMemoryExtractionJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartMemoryExtractionJob<Eng>
{
    type Input = StartMemoryExtractionJobInput;
    type Output = StartMemoryExtractionJobOutput;
    type Error = StartMemoryExtractionJobError;

    const NAME: &'static str = "StartMemoryExtractionJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_MEMORY_EXTRACTION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_MEMORY_EXTRACTION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Request metadata is not read by this operation: every member it needs is
        // bound into `input` by the centralized request walk.
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            StartMemoryExtractionJobErrorModeled::ServiceException(ServiceException {
                message: "the shared engine does not carry this service's state".to_string(),
            })
        })?;

        // The same pre-condition every call on a container's stored memory ITEMS keeps,
        // worded the way those surfaces word that level: a Memory container is created
        // through the CONTROL plane, so the container on this path is resolved against the
        // component that owns it, before this service's own store is taken.
        crate::state::memory_item_container_resolved(&*self.engine, &input.memory_id).map_err(
            |missing| match missing {
                Some(message) => StartMemoryExtractionJobErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(message),
                    },
                ),
                None => {
                    StartMemoryExtractionJobErrorModeled::ServiceException(ServiceException {
                        message:
                            "the shared engine does not carry the AgentCore resource inventory"
                                .to_string(),
                    })
                }
            },
        )?;

        let job_id = input.extraction_job.job_id;
        // The strategies the extraction this start asks for runs under, read off the
        // CONTROL plane that owns them and released before this service's own store is
        // taken, which is the lock order every reader of another plane's store keeps.
        let strategies = crate::state::memory_extraction_strategies(&*self.engine, &input.memory_id);
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // The pick-up: the messages of this unit the container has accepted and no
        // earlier attempt covered, extracted into the records those strategies derive from
        // them. An id naming no unit of work and no job an earlier start picked up
        // addresses no extraction job at all.
        let started = store
            .start_extraction_job(&input.memory_id, &job_id, &strategies, now)
            .ok_or_else(|| {
                StartMemoryExtractionJobErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::resource_not_found_message(
                            crate::state::NotFoundResource::ExtractionJob,
                            &job_id,
                        )),
                    },
                )
            })?;

        Ok(StartMemoryExtractionJobOutput {
            job_id: started.job_id,
        })
    }
}
