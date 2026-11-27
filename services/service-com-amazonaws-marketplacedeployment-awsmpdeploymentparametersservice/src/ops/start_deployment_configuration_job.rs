// SPDX-License-Identifier: CC0-1.0

//! @op StartDeploymentConfigurationJob
//!
//! Starts the JOB that publishes a deployment configuration for a product's
//! delivery option, and reports it back from the record it wrote.
//!
//! The publish is ASYNCHRONOUS and this operation is the start of it: the model
//! documents that the job "runs asynchronously. Use GetDeploymentConfigurationJob
//! to track its status. When the job reaches the SUCCEEDED status, its
//! configuration becomes the active deployment configuration for the product and
//! delivery option", and that the status this operation answers with "is
//! IN_PROGRESS". So what a success here means is a job that EXISTS
//! ([`crate::state::deployment_configuration::DeploymentConfigurationJobRecord`]),
//! not a configuration that is already active: the terminal status the clock owes
//! it becomes due a span later
//! ([`crate::state::deployment_configuration::JOB_SETTLE_DELAY`]) and the operations
//! that resolve a job or a configuration materialise it, so a caller that reads the
//! job straight back sees the `IN_PROGRESS` status it was told about.
//!
//! What the job will DO is decided here, from the configuration alone
//! ([`crate::state::deployment_configuration::judge_configuration`]) and recorded
//! with it, so the answer does not depend on when anybody looks: a fulfillment URL
//! the service will not publish fails the job with the modelled error code rather
//! than being refused at the request, because that code is what the model documents
//! the condition as (`DeploymentConfigurationJobErrorCode$ENDPOINT_INVALID`) and
//! `DeploymentConfigurationFulfillmentUrl` carries no `@pattern` for the modelled
//! walk to refuse it with.
//!
//! What IS refused here, in the order the service applies it:
//!
//! 1. a `catalog` naming no catalog this service serves
//!    ([`crate::state::SUPPORTED_CATALOGS`]) — the modelled `ValidationException`,
//!    judged before any state is touched, exactly as `PutDeploymentParameter`
//!    judges it;
//! 2. a `configuration` whose populated member is not the configuration of the
//!    `deliveryOptionType` the path names: the model's `DeploymentConfigurationDetail`
//!    states that "exactly one member must be set, and it must match the delivery
//!    option type", and this operation's own doc repeats it — the modelled
//!    `ValidationException`, blamed on `configuration`;
//! 3. a `tags` map outside what a tag set of this service may hold, judged on the
//!    INPUT through the same rules every other tagging member of this service is
//!    judged by ([`crate::state::validate_tags_member`] with the constraints of the
//!    modelled `TagsMap` this member targets);
//! 4. a product the account does not hold in that catalog — the modelled
//!    `ResourceNotFoundException`, blamed on the path segment that did not resolve
//!    ([`crate::state::target_not_found`]): a product exists because a seller
//!    published it, out of band of this API, so it is a PRECONDITION this operation
//!    resolves and never one the start establishes;
//! 5. a `clientToken` already used to start a DIFFERENT job — the modelled
//!    `ConflictException`. The member is the `@idempotencyToken`: "if you retry a
//!    request with the same token and the same input, the service returns the
//!    original job instead of starting a new one. If you reuse a token with
//!    different input, the request fails with a ConflictException";
//! 6. `tags` on a start for a deployment configuration that ALREADY exists — the
//!    modelled `ValidationException`
//!    ([`crate::state::deployment_configuration::tags_on_existing_configuration`]):
//!    "only the request that creates the deployment configuration can apply tags …
//!    To change tags on an existing deployment configuration, use TagResource and
//!    UntagResource". The replay above is answered first, so a retry of the create
//!    that applied the tags still replays rather than being refused.
//!
//! ---
//!
//! # Modeled contract for `StartDeploymentConfigurationJob`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")
//! ```
//!
//! Starts a job that publishes a deployment configuration for a product and delivery
//! option. The job runs asynchronously. Use GetDeploymentConfigurationJob to track its
//! status. When the job reaches the SUCCEEDED status, its configuration becomes the active
//! deployment configuration for the product and delivery option. The populated member of
//! configuration must match the deliveryOptionType in the request path.
//!
//! ## Input
//!
//! * `catalog` (Catalog, len 1..64, pattern ^[a-zA-Z_-]+$) **required** — The catalog related
//!   to the request. Fixed value: AWSMarketplace
//! * `clientToken` (ClientToken, len 32..64, pattern ^[a-zA-Z0-9/_+=.:@-]+$,
//!   @idempotencyToken) — A unique, case-sensitive identifier that you provide to ensure that
//!   the operation completes no more than one time. If you retry a request with the same
//!   token and the same input, the service returns the original job instead of starting a new
//!   one. If you reuse a token with different input, the request fails with a
//!   ConflictException. This field is not required if you're calling using an AWS SDK.
//!   Otherwise, a clientToken must be provided with the request.
//! * `configuration` (DeploymentConfigurationDetail) **required** — The deployment
//!   configuration to publish.
//! * `deliveryOptionType` (DeliveryOptionType, one of: data-as-a-service) **required** — The
//!   delivery option type of the deployment configuration.
//! * `productId` (ResourceId, len 1..64, pattern ^[A-Za-z0-9_/-]+$) **required** — The ID of
//!   the AWS Marketplace product, for example prod-1234567890abc.
//! * `tags` (TagsMap, len ..50) — A map of key-value pairs, where each pair represents a tag
//!   to apply to the deployment configuration. Only the request that creates the deployment
//!   configuration can apply tags. If the deployment configuration already exists, a request
//!   that includes one or more tags fails with a ValidationException. To change tags on an
//!   existing deployment configuration, use TagResource and UntagResource.
//!
//! ## Output
//!
//! * `configuration` (DeploymentConfigurationDetail) **required** — The deployment
//!   configuration submitted with the job.
//! * `createdAt` (Timestamp) **required** — The date and time when the job was created.
//! * `jobId` (DeploymentConfigurationJobId, len 1..64, pattern ^[a-z0-9]+$) **required** —
//!   The unique identifier of the deployment configuration job.
//! * `status` (DeploymentConfigurationJobStatus, one of: FAILED | IN_PROGRESS | SUCCEEDED)
//!   **required** — The status of the deployment configuration job. The value is IN_PROGRESS.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `ConflictException` (HTTP 409) — The request configuration has conflicts. For details,
//!   see the accompanying error message.
//! * `InternalServerException` (HTTP 500) — There was an internal service exception.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource wasn't found.
//! * `ThrottlingException` (HTTP 429) — Too many requests.
//! * `ValidationException` (HTTP 400) — An error occurred during validation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `StartDeploymentConfigurationJobError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `StartDeploymentConfigurationJobError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.marketplacedeployment#StartDeploymentConfigurationJob")]
#[derive(Debug)]
pub enum StartDeploymentConfigurationJobErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::start_deployment_configuration_job::StartDeploymentConfigurationJobError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type StartDeploymentConfigurationJobError = crate::scaffolded::Scaffolded<StartDeploymentConfigurationJobErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<StartDeploymentConfigurationJobErrorModeled> for StartDeploymentConfigurationJobError {
    fn from(modeled: StartDeploymentConfigurationJobErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `StartDeploymentConfigurationJob` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct StartDeploymentConfigurationJob<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> StartDeploymentConfigurationJob<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for StartDeploymentConfigurationJob<Eng>
{
    type Input = StartDeploymentConfigurationJobInput;
    type Output = StartDeploymentConfigurationJobOutput;
    type Error = StartDeploymentConfigurationJobError;

    const NAME: &'static str = "StartDeploymentConfigurationJob";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_DEPLOYMENT_CONFIGURATION_JOB_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::START_DEPLOYMENT_CONFIGURATION_JOB_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The CATALOG first, before any state is touched, for the reason
        // `PutDeploymentParameter` gives: the model documents the member as a fixed
        // value, so a syntactically valid name outside the set of catalogs that exist
        // names no catalog at all and there is nothing to publish inside it.
        if !crate::state::is_supported_catalog(&input.catalog) {
            return Err(StartDeploymentConfigurationJobErrorModeled::ValidationException(
                crate::state::unsupported_catalog(),
            )
            .into());
        }

        // Then the request's own INTERNAL consistency: the union carries the
        // configuration of ONE delivery option type and the path names the delivery
        // option this job is for, so a configuration of another one is a request that
        // cannot be served whatever the store holds.
        let populated =
            crate::state::deployment_configuration::delivery_option_type_of(&input.configuration);
        if populated != input.delivery_option_type {
            return Err(StartDeploymentConfigurationJobErrorModeled::ValidationException(
                crate::state::deployment_configuration::configuration_delivery_option_mismatch(
                    &input.delivery_option_type,
                    populated,
                ),
            )
            .into());
        }

        // Then the TAGS, still on the input alone: a tag key or value outside what a
        // tag of this service may hold is a bad request whether this start would
        // apply the set or be refused for carrying it at all, and a map past the
        // number of tags one resource holds is the tagging front end's own refusal.
        // The member targets the modelled `TagsMap`, so its character class holds
        // here — unlike `TagResource`'s, which targets the map the model leaves
        // unconstrained.
        let incoming = input.tags.clone().unwrap_or_default();
        if !incoming.is_empty() {
            if let Err(why) = crate::state::validate_tags_member(
                &incoming,
                crate::state::TagConstraints::TagsMap,
            ) {
                return Err(why
                    .into_error(StartDeploymentConfigurationJobErrorModeled::ValidationException));
            }
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(StartDeploymentConfigurationJobErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());

        let mut store = state.lock();
        // ONE clock sample for the whole call, taken once the store is held: the
        // transitions the clock has already made due are materialised with it, and the
        // job this call records is created at it.
        let now = ::std::time::SystemTime::now();
        store.settle_due_jobs(now);

        // The resource the request's URI PATH addresses. A product exists because a
        // seller published it, out of band of this API, so this is resolved and never
        // established here.
        if !store.holds_product(&input.catalog, &input.product_id) {
            return Err(StartDeploymentConfigurationJobErrorModeled::ResourceNotFoundException(
                crate::state::target_not_found("ProductId"),
            )
            .into());
        }

        let key = (
            input.catalog.clone(),
            input.product_id.clone(),
            input.delivery_option_type.clone(),
        );
        let tags: ::std::collections::BTreeMap<String, String> = incoming
            .iter()
            .map(|(tag_key, value)| (tag_key.clone(), value.clone()))
            .collect();

        // The `@idempotencyToken` replay: a token identifies ONE start. The same
        // request through it answers with the job it already started, read out of the
        // record; a different request through it is a conflict and starts nothing.
        if let Some(client_token) = input.client_token.as_deref() {
            if let Some(job) = store.configuration_job_for_client_token(client_token) {
                let same_request = job.key() == key
                    && crate::state::deployment_configuration::configuration_fingerprint(
                        &job.configuration,
                    ) == crate::state::deployment_configuration::configuration_fingerprint(
                        &input.configuration,
                    )
                    && job.tags == tags;
                if !same_request {
                    return Err(StartDeploymentConfigurationJobErrorModeled::ConflictException(
                        crate::state::client_token_conflict(client_token, &job.job_id),
                    )
                    .into());
                }
                return Ok(reported(job));
            }
        }

        // Tags apply to the deployment configuration, and only the start that CREATES
        // it may carry them.
        if !tags.is_empty() && store.holds_configuration(&key) {
            return Err(StartDeploymentConfigurationJobErrorModeled::ValidationException(
                crate::state::deployment_configuration::tags_on_existing_configuration(),
            )
            .into());
        }

        let job_id = store.mint_job_id();
        let outcome =
            crate::state::deployment_configuration::judge_configuration(&input.configuration);
        store.record_configuration_job(
            crate::state::deployment_configuration::DeploymentConfigurationJobRecord {
                job_id: job_id.clone(),
                catalog: input.catalog.clone(),
                product_id: input.product_id.clone(),
                delivery_option_type: input.delivery_option_type.clone(),
                region,
                configuration: input.configuration.clone(),
                tags,
                client_token: input.client_token.clone(),
                status: crate::state::deployment_configuration::JOB_STATUS_IN_PROGRESS.to_string(),
                outcome,
                created_at: now,
                settles_at: now
                    .checked_add(crate::state::deployment_configuration::JOB_SETTLE_DELAY)
                    .unwrap_or(now),
                ended_at: None,
            },
        );

        // Reported from the STORED record, so what the caller is told is what a later
        // `GetDeploymentConfigurationJob` will answer with and never the request
        // echoed back.
        let Some(job) = store.configuration_job(&job_id) else {
            return Err(StartDeploymentConfigurationJobErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
            .into());
        };
        Ok(reported(job))
    }
}

/// This operation's answer, read off the stored job.
fn reported(
    job: &crate::state::deployment_configuration::DeploymentConfigurationJobRecord,
) -> StartDeploymentConfigurationJobOutput {
    StartDeploymentConfigurationJobOutput {
        job_id: job.job_id.clone(),
        configuration: job.configuration.clone(),
        status: job.status.clone(),
        created_at: ::aws_smithy_types::DateTime::from(job.created_at),
    }
}
