// SPDX-License-Identifier: CC0-1.0

//! @op ListDeploymentConfigurationJobs
//!
//! Reports the deployment configuration jobs started for a product's delivery
//! option, newest first, one page at a time — every field read out of the stored
//! job records.
//!
//! The order is the model's: the jobs are "ordered from newest to oldest"
//! ([`crate::state::ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::configuration_jobs_newest_first`]
//! states it once, so this operation and any later reader cannot disagree), and the
//! statuses reported are the ones the clock has already made due, materialised
//! before anything is read for the reason
//! [`crate::ops::get_deployment_configuration_job`] gives.
//!
//! A product with no jobs is an EMPTY list and not a failure — "if you haven't
//! started any jobs for the product and delivery option, the operation returns an
//! empty jobs list" — which is also why this operation, alone among the four that
//! address a deployment configuration, models no `ResourceNotFoundException`: there
//! is no resource it has to resolve beyond the page itself.
//!
//! PAGINATION is the model's `@paginated(inputToken: nextToken, outputToken:
//! nextToken, pageSize: maxResults, items: jobs)`. A page holds at most
//! `maxResults` jobs, whose modelled default is
//! [`crate::state::deployment_configuration::MAX_RESULTS_DEFAULT`], and the
//! `nextToken` answered beside it is the job the NEXT page starts at — so reading a
//! sequence of pages walks the order above exactly once, and the member is absent on
//! the last page, where the model says "this value is absent when there are no more
//! results". A token of any other provenance names no page in that order and is the
//! modelled `ValidationException`
//! ([`crate::state::deployment_configuration::invalid_next_token`]).
//!
//! ---
//!
//! # Modeled contract for `ListDeploymentConfigurationJobs`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")
//! ```
//!
//! Lists the deployment configuration jobs that you started for a product and delivery
//! option, ordered from newest to oldest. Results are paginated. Use the nextToken
//! parameter to retrieve additional results. If you haven't started any jobs for the
//! product and delivery option, the operation returns an empty jobs list.
//!
//! ## Input
//!
//! * `catalog` (Catalog, len 1..64, pattern ^[a-zA-Z_-]+$) **required** — The catalog related
//!   to the request. Fixed value: AWSMarketplace
//! * `deliveryOptionType` (DeliveryOptionType, one of: data-as-a-service) **required** — The
//!   delivery option type of the deployment configuration.
//! * `maxResults` (DeploymentConfigurationMaxResults, range 1..100, default 20) **required**
//!   — The maximum number of jobs to return in a single page. Valid values are 1 through 100.
//!   The default is 20.
//! * `nextToken` (DeploymentConfigurationNextToken, len 1..2048) — The token for the next
//!   page of results. Use the value returned in the previous response.
//! * `productId` (ResourceId, len 1..64, pattern ^[A-Za-z0-9_/-]+$) **required** — The ID of
//!   the AWS Marketplace product, for example prod-1234567890abc.
//!
//! ## Output
//!
//! * `jobs` (DeploymentConfigurationJobSummaries) **required** — A list of deployment
//!   configuration job summaries, ordered from newest to oldest.
//! * `nextToken` (DeploymentConfigurationNextToken, len 1..2048) — The token for the next
//!   page of results. This value is absent when there are no more results.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — There was an internal service exception.
//! * `ThrottlingException` (HTTP 429) — Too many requests.
//! * `ValidationException` (HTTP 400) — An error occurred during validation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListDeploymentConfigurationJobsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListDeploymentConfigurationJobsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.marketplacedeployment#ListDeploymentConfigurationJobs")]
#[derive(Debug)]
pub enum ListDeploymentConfigurationJobsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_deployment_configuration_jobs::ListDeploymentConfigurationJobsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListDeploymentConfigurationJobsError = crate::scaffolded::Scaffolded<ListDeploymentConfigurationJobsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListDeploymentConfigurationJobsErrorModeled> for ListDeploymentConfigurationJobsError {
    fn from(modeled: ListDeploymentConfigurationJobsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListDeploymentConfigurationJobs` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListDeploymentConfigurationJobs<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListDeploymentConfigurationJobs<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListDeploymentConfigurationJobs<Eng>
{
    type Input = ListDeploymentConfigurationJobsInput;
    type Output = ListDeploymentConfigurationJobsOutput;
    type Error = ListDeploymentConfigurationJobsError;

    const NAME: &'static str = "ListDeploymentConfigurationJobs";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_DEPLOYMENT_CONFIGURATION_JOBS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_DEPLOYMENT_CONFIGURATION_JOBS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The CATALOG first, before any state is touched: the member is documented as
        // a fixed value, so a name outside the set of catalogs that exist names no
        // catalog whose jobs could be listed.
        if !crate::state::is_supported_catalog(&input.catalog) {
            return Err(ListDeploymentConfigurationJobsErrorModeled::ValidationException(
                crate::state::unsupported_catalog(),
            )
            .into());
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(ListDeploymentConfigurationJobsErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
            .into());
        };

        let mut store = state.lock();
        // ONE clock sample, taken once the store is held: every status this page
        // reports is the one owed at that instant, so two jobs of one page are never
        // judged against two different clocks.
        let now = ::std::time::SystemTime::now();
        store.settle_due_jobs(now);

        let key = (
            input.catalog.clone(),
            input.product_id.clone(),
            input.delivery_option_type.clone(),
        );
        let ordered = store.configuration_jobs_newest_first(&key);

        // Where this page starts: the job the previous page's `nextToken` named, or
        // the newest job when the request carries none.
        let start = match input.next_token.as_deref() {
            Some(token) => {
                let Some(index) = ordered.iter().position(|job| job.job_id == token) else {
                    return Err(ListDeploymentConfigurationJobsErrorModeled::ValidationException(
                        crate::state::deployment_configuration::invalid_next_token(),
                    )
                    .into());
                };
                index
            }
            None => 0,
        };

        let page_size = page_size(input.max_results);
        let jobs: Vec<DeploymentConfigurationJobSummary> = ordered
            .iter()
            .skip(start)
            .take(page_size)
            .map(|job| DeploymentConfigurationJobSummary {
                job_id: job.job_id.clone(),
                status: job.status.clone(),
                configuration: job.configuration.clone(),
                created_at: ::aws_smithy_types::DateTime::from(job.created_at),
            })
            .collect();
        // The token is the job the next page starts at, so it is absent exactly when
        // this page reached the end of the order.
        let next_token = ordered
            .get(start + jobs.len())
            .map(|job| job.job_id.clone());

        Ok(ListDeploymentConfigurationJobsOutput { jobs, next_token })
    }
}

/// How many jobs this page holds.
///
/// `maxResults` is "the maximum number of jobs to return in a single page. Valid
/// values are 1 through 100. The default is 20", and the model carries that default
/// and that range — so a value outside the range cannot reach this handler (the
/// modelled constraint walk refuses it) and an absent member arrives as the default.
/// The modelled default therefore stands in for the one value left, a member the
/// protocol could not fill.
fn page_size(max_results: i32) -> usize {
    let requested = usize::try_from(max_results).unwrap_or(0);
    let modelled = 1..=crate::state::deployment_configuration::MAX_RESULTS_MAX;
    if modelled.contains(&requested) {
        requested
    } else {
        crate::state::deployment_configuration::MAX_RESULTS_DEFAULT
    }
}
