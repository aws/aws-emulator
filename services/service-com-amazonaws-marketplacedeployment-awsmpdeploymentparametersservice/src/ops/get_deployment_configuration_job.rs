// SPDX-License-Identifier: CC0-1.0

//! @op GetDeploymentConfigurationJob
//!
//! Reports one deployment configuration job, out of the record
//! `StartDeploymentConfigurationJob` wrote.
//!
//! This is the operation the model points a client at to "track its status", so the
//! status it reports is the one the CLOCK has already made due: the job "runs
//! asynchronously" and its terminal status becomes due a span after the start
//! ([`crate::state::deployment_configuration::JOB_SETTLE_DELAY`]), so this call
//! materialises that transition before it resolves anything
//! ([`crate::state::ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::settle_due_jobs`])
//! and then only READS. The transition is one the clock owed at the same instant
//! whether or not anybody polled — which is why a reporting operation may apply it —
//! and `endedAt` is that instant and not the instant of the call, so two callers
//! reading one job are told it ended at one time.
//!
//! Three members are reported only on the status the model binds them to:
//! `configurationArn` "is present only when the job status is SUCCEEDED" — and it is
//! the ARN of the configuration record that job PUBLISHED, read from that record
//! rather than built here — while `errorCode` and `message` are "present only when
//! the job status is FAILED".
//!
//! A job is addressed INSIDE the resource the path names, so the request resolves
//! that resource first: an unheld product is the modelled
//! `ResourceNotFoundException` blamed on `productId`, and a `jobId` that names no job
//! — or one belonging to another product or delivery option, which the model's own
//! doc calls out — is the same failure blamed on `jobId`
//! ([`crate::state::target_not_found`]). It is the modelled shape here, not the
//! `NotFoundException` code the tagging operations answer: that code belongs to the
//! tagging front end's resolution of a caller-supplied resource ARN, while this
//! operation's own model declares the shape for the resources its PATH addresses.
//!
//! ---
//!
//! # Modeled contract for `GetDeploymentConfigurationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")
//! ```
//!
//! Returns the details of a deployment configuration job. Only the seller that started the
//! job can call this operation. If the job doesn't belong to the product and delivery
//! option in the request path, the operation returns a ResourceNotFoundException.
//!
//! ## Input
//!
//! * `catalog` (Catalog, len 1..64, pattern ^[a-zA-Z_-]+$) **required** — The catalog related
//!   to the request. Fixed value: AWSMarketplace
//! * `deliveryOptionType` (DeliveryOptionType, one of: data-as-a-service) **required** — The
//!   delivery option type of the deployment configuration.
//! * `jobId` (DeploymentConfigurationJobId, len 1..64, pattern ^[a-z0-9]+$) **required** —
//!   The unique identifier of the deployment configuration job.
//! * `productId` (ResourceId, len 1..64, pattern ^[A-Za-z0-9_/-]+$) **required** — The ID of
//!   the AWS Marketplace product, for example prod-1234567890abc.
//!
//! ## Output
//!
//! * `configuration` (DeploymentConfigurationDetail) **required** — The deployment
//!   configuration submitted with the job.
//! * `configurationArn` (ResourceArn, len 1..2048, pattern ^[a-zA-Z0-9:*/-]+$) — The Amazon
//!   Resource Name (ARN) of the deployment configuration. This field is present only when the
//!   job status is SUCCEEDED.
//! * `createdAt` (Timestamp) **required** — The date and time when the job was created.
//! * `endedAt` (Timestamp) — The date and time when the job reached the SUCCEEDED or FAILED
//!   status.
//! * `errorCode` (DeploymentConfigurationJobErrorCode, one of: ENDPOINT_INVALID |
//!   FULFILLMENT_URL_UNREACHABLE | INTERNAL_ERROR) — The error code that describes why the
//!   job failed. This field is present only when the job status is FAILED.
//! * `jobId` (DeploymentConfigurationJobId, len 1..64, pattern ^[a-z0-9]+$) **required** —
//!   The unique identifier of the deployment configuration job.
//! * `message` (String) — A message that describes why the job failed. This field is present
//!   only when the job status is FAILED.
//! * `status` (DeploymentConfigurationJobStatus, one of: FAILED | IN_PROGRESS | SUCCEEDED)
//!   **required** — The status of the deployment configuration job.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — There was an internal service exception.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource wasn't found.
//! * `ThrottlingException` (HTTP 429) — Too many requests.
//! * `ValidationException` (HTTP 400) — An error occurred during validation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetDeploymentConfigurationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetDeploymentConfigurationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.marketplacedeployment#GetDeploymentConfigurationJob")]
#[derive(Debug)]
pub enum GetDeploymentConfigurationJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_deployment_configuration_job::GetDeploymentConfigurationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetDeploymentConfigurationJobError = crate::scaffolded::Scaffolded<GetDeploymentConfigurationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetDeploymentConfigurationJobErrorModeled> for GetDeploymentConfigurationJobError {
    fn from(modeled: GetDeploymentConfigurationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDeploymentConfigurationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetDeploymentConfigurationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDeploymentConfigurationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDeploymentConfigurationJob<Eng>
{
    type Input = GetDeploymentConfigurationJobInput;
    type Output = GetDeploymentConfigurationJobOutput;
    type Error = GetDeploymentConfigurationJobError;

    const NAME: &'static str = "GetDeploymentConfigurationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DEPLOYMENT_CONFIGURATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DEPLOYMENT_CONFIGURATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The CATALOG first, before any state is touched: the member is documented as
        // a fixed value, so a name outside the set of catalogs that exist names no
        // catalog to look inside.
        if !crate::state::is_supported_catalog(&input.catalog) {
            return Err(GetDeploymentConfigurationJobErrorModeled::ValidationException(
                crate::state::unsupported_catalog(),
            )
            .into());
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetDeploymentConfigurationJobErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
            .into());
        };

        let mut store = state.lock();
        // ONE clock sample, taken once the store is held: the terminal status this
        // call reports is the one owed at that instant, and nothing is slept for.
        let now = ::std::time::SystemTime::now();
        store.settle_due_jobs(now);

        if !store.holds_product(&input.catalog, &input.product_id) {
            return Err(GetDeploymentConfigurationJobErrorModeled::ResourceNotFoundException(
                crate::state::target_not_found("ProductId"),
            )
            .into());
        }

        let key = (
            input.catalog.clone(),
            input.product_id.clone(),
            input.delivery_option_type.clone(),
        );
        // A job of another product or delivery option is not this resource's job, so
        // it is reported as no job at all — the model says so in as many words.
        let Some(job) = store
            .configuration_job(&input.job_id)
            .filter(|job| job.key() == key)
        else {
            return Err(GetDeploymentConfigurationJobErrorModeled::ResourceNotFoundException(
                crate::state::target_not_found("JobId"),
            )
            .into());
        };

        // The ARN comes from the configuration the job PUBLISHED, which is the record
        // that owns it: a job that has not succeeded published none, and so reports
        // none.
        let configuration_arn = job.has_succeeded().then(|| {
            store
                .active_configuration(&key)
                .map(|configuration| configuration.configuration_arn.clone())
        });

        Ok(GetDeploymentConfigurationJobOutput {
            job_id: job.job_id.clone(),
            configuration: job.configuration.clone(),
            status: job.status.clone(),
            created_at: ::aws_smithy_types::DateTime::from(job.created_at),
            configuration_arn: configuration_arn.flatten(),
            ended_at: job.ended_at.map(::aws_smithy_types::DateTime::from),
            error_code: job.error_code().map(str::to_string),
            message: job.message().map(str::to_string),
        })
    }
}
