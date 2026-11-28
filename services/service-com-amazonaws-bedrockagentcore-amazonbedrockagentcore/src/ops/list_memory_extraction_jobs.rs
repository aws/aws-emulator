// SPDX-License-Identifier: CC0-1.0

//! @op ListMemoryExtractionJobs
//! Reports the units of long-term memory extraction work a memory container holds — the
//! extraction jobs eligible to be started — computed from what that container has
//! accepted for extraction.
//!
//! The rule: one job per SESSION of one actor, composed of the messages that session's
//! records carry for long-term extraction — the conversational turns of the events
//! `CreateEvent` stored in it (an event created with `extractionMode` `SKIP` is excluded
//! from extraction, so it composes none) and of the content `IngestData` submitted into
//! it — each message named by the id of the record it belongs to and its position in
//! that record's messages; a session composing no message is no unit of work and is not
//! reported. The optional `filter` narrows the list by every member it carries at once,
//! and the page is bounded by `maxResults` with an opaque `nextToken` resuming after it.
//! Nothing is reported about an extraction ATTEMPT: no `status`, no `failureReason`.
//!
//! The derivation itself lives beside the store it reads
//! ([`crate::state::ComAmazonawsBedrockagentcoreAmazonbedrockagentcoreState::extraction_job_units`]),
//! so the `jobId` this listing reports and the unit it stands for have one home.
//!
//! This operation only REPORTS: it writes nothing, so two listings of an unchanged
//! container answer the same.
//!
//! ---
//!
//! # Modeled contract for `ListMemoryExtractionJobs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json")
//! ```
//!
//! Lists all long-term memory extraction jobs that are eligible to be started with optional
//! filtering. To use this operation, you must have the
//! bedrock-agentcore:ListMemoryExtractionJobs permission.
//!
//! ## Input
//!
//! * `filter` (ExtractionJobFilterInput) — Filter criteria to apply when listing extraction
//!   jobs.
//! * `maxResults` (Integer, default 20) **required** — The maximum number of results to
//!   return in a single call. The default value is 20.
//! * `memoryId` (MemoryId, len 12.., pattern
//!   ^(arn:(aws|aws-cn|aws-us-gov):bedrock-agentcore:[a-z0-9-]+:[0-9]{12}:memory/)?[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10
//!   …) **required** — The unique identifier of the memory to list extraction jobs for.
//! * `nextToken` (PaginationToken) — The token for the next set of results. Use the value
//!   returned in the previous response in the next request to retrieve the next set of
//!   results.
//!
//! ## Output
//!
//! * `jobs` (ExtractionJobMetadataList) **required** — List of extraction job metadata
//!   matching the specified criteria.
//! * `nextToken` (PaginationToken) — Token to retrieve the next page of results, if
//!   available.
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

/// The list this operation's `nextToken` belongs to, so a token minted by another
/// paginated operation does not silently resume this one.
const PAGE_KIND: &str = "extractionJobs";

/// Whether a job the store holds satisfies one member of the filter: a member the
/// request left out narrows nothing, and one it carries keeps only the jobs reporting
/// exactly that value — so a filter on a member no job carries keeps none of them.
fn member_matches(wanted: Option<&String>, held: Option<&String>) -> bool {
    wanted.is_none_or(|wanted| held.is_some_and(|held| held == wanted))
}

/// Whether `job` satisfies the whole filter: each member the request carried is
/// documented as "only extraction jobs with this \<member\> are returned", so several
/// members narrow TOGETHER.
fn keeps(filter: &ExtractionJobFilterInput, job: &ExtractionJobMetadata) -> bool {
    member_matches(filter.actor_id.as_ref(), job.actor_id.as_ref())
        && member_matches(filter.session_id.as_ref(), job.session_id.as_ref())
        && member_matches(filter.strategy_id.as_ref(), job.strategy_id.as_ref())
        && member_matches(filter.status.as_ref(), job.status.as_ref())
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListMemoryExtractionJobsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListMemoryExtractionJobsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentcore#ListMemoryExtractionJobs")]
#[derive(Debug)]
pub enum ListMemoryExtractionJobsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceException(ServiceException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottledException(ThrottledException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_memory_extraction_jobs::ListMemoryExtractionJobsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListMemoryExtractionJobsError = crate::scaffolded::Scaffolded<ListMemoryExtractionJobsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListMemoryExtractionJobsErrorModeled> for ListMemoryExtractionJobsError {
    fn from(modeled: ListMemoryExtractionJobsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListMemoryExtractionJobs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListMemoryExtractionJobs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListMemoryExtractionJobs<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListMemoryExtractionJobs<Eng>
{
    type Input = ListMemoryExtractionJobsInput;
    type Output = ListMemoryExtractionJobsOutput;
    type Error = ListMemoryExtractionJobsError;

    const NAME: &'static str = "ListMemoryExtractionJobs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MEMORY_EXTRACTION_JOBS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_MEMORY_EXTRACTION_JOBS_OUTPUT_SCHEMA;
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
            ListMemoryExtractionJobsErrorModeled::ServiceException(ServiceException {
                message: "the shared engine does not carry this service's state".to_string(),
            })
        })?;

        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => crate::state::decode_next_token(PAGE_KIND, token).ok_or_else(|| {
                ListMemoryExtractionJobsErrorModeled::ValidationException(ValidationException {
                    message: "nextToken is not a token this operation issued".to_string(),
                    reason: "CannotParse".to_string(),
                    field_list: None,
                })
            })?,
        };
        // `@range(1..50)` is refused at the boundary; the clamp keeps this arithmetic
        // total rather than restating that refusal.
        let max_results = input.max_results.clamp(1, 50) as usize;

        // The same pre-condition every call addressing something inside a container
        // keeps: a Memory container is created through the CONTROL plane, so the one
        // addressed resource on this path is resolved against the component that owns it,
        // before this service's own store is taken. The extraction jobs are composed over
        // the container's stored memory ITEMS, so this refusal is worded the way those
        // surfaces word an absent container — after the memory-item store and with no part
        // of the request in it — and not after the container as the event surfaces word it.
        crate::state::memory_item_container_resolved(&*self.engine, &input.memory_id).map_err(
            |missing| match missing {
                Some(message) => ListMemoryExtractionJobsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(message),
                    },
                ),
                None => ListMemoryExtractionJobsErrorModeled::ServiceException(ServiceException {
                    message: "the shared engine does not carry the AgentCore resource inventory"
                        .to_string(),
                }),
            },
        )?;

        // The extraction the clock has already made due is materialised before the units
        // are composed: a unit is eligible to be STARTED exactly while no extraction has
        // picked its messages up, so one the automatic pipeline has already extracted is
        // no longer work. The strategies come from the CONTROL plane that owns them, read
        // and released before this service's own store is taken.
        let strategies = crate::state::memory_extraction_strategies(&*self.engine, &input.memory_id);
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        store.settle_long_term_extraction(&input.memory_id, &strategies, now);
        // The units of extraction work the container holds, oldest first, rendered as the
        // modelled metadata so the filter reads exactly the members a caller sees.
        let mut jobs: Vec<ExtractionJobMetadata> = store
            .extraction_job_units(&input.memory_id)
            .iter()
            .map(crate::state::ExtractionJobUnit::to_metadata)
            .collect();
        if let Some(filter) = input.filter.as_ref() {
            jobs.retain(|job| keeps(filter, job));
        }

        let (jobs, next_token) =
            crate::state::paginate_with(&jobs, PAGE_KIND, offset, max_results, Clone::clone);

        Ok(ListMemoryExtractionJobsOutput { jobs, next_token })
    }
}
