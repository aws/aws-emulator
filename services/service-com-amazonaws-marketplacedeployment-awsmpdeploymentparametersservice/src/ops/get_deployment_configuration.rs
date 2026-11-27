// SPDX-License-Identifier: CC0-1.0

//! @op GetDeploymentConfiguration
//!
//! Reports the ACTIVE deployment configuration of a product's delivery option, out
//! of the record the job that published it wrote.
//!
//! "The active deployment configuration is the configuration from the most recent
//! successful deployment configuration job", so this operation reports no
//! configuration of its own: it reads the one record a succeeded job left
//! ([`crate::state::deployment_configuration::DeploymentConfigurationRecord`]), and
//! a resource no job has published a configuration for "returns a
//! ResourceNotFoundException" — the model says so in as many words. The transition
//! that MAKES a configuration active is one the clock owes a started job, so it is
//! materialised before anything is read
//! ([`crate::state::ComAmazonawsMarketplacedeploymentAwsmpdeploymentparametersserviceState::settle_due_jobs`]):
//! the configuration a job published becomes readable here exactly when that job's
//! `SUCCEEDED` status was due, and never before.
//!
//! The not-found is blamed on the segment of the path that did not resolve
//! ([`crate::state::target_not_found`]): the product the path addresses when the
//! account holds no such product, and otherwise the delivery option, whose
//! configuration is what the request came for.
//!
//! ---
//!
//! # Modeled contract for `GetDeploymentConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json")
//! ```
//!
//! Returns the active deployment configuration for a product and delivery option. The
//! active deployment configuration is the configuration from the most recent successful
//! deployment configuration job. The seller that owns the product and buyers entitled to
//! the product can call this operation. If the product and delivery option have no active
//! deployment configuration, the operation returns a ResourceNotFoundException.
//!
//! ## Input
//!
//! * `catalog` (Catalog, len 1..64, pattern ^[a-zA-Z_-]+$) **required** — The catalog related
//!   to the request. Fixed value: AWSMarketplace
//! * `deliveryOptionType` (DeliveryOptionType, one of: data-as-a-service) **required** — The
//!   delivery option type of the deployment configuration.
//! * `productId` (ResourceId, len 1..64, pattern ^[A-Za-z0-9_/-]+$) **required** — The ID of
//!   the AWS Marketplace product, for example prod-1234567890abc.
//!
//! ## Output
//!
//! * `configuration` (DeploymentConfigurationDetail) **required** — The active deployment
//!   configuration.
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
// `GetDeploymentConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetDeploymentConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.marketplacedeployment#GetDeploymentConfiguration")]
#[derive(Debug)]
pub enum GetDeploymentConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_deployment_configuration::GetDeploymentConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetDeploymentConfigurationError = crate::scaffolded::Scaffolded<GetDeploymentConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetDeploymentConfigurationErrorModeled> for GetDeploymentConfigurationError {
    fn from(modeled: GetDeploymentConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDeploymentConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetDeploymentConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDeploymentConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDeploymentConfiguration<Eng>
{
    type Input = GetDeploymentConfigurationInput;
    type Output = GetDeploymentConfigurationOutput;
    type Error = GetDeploymentConfigurationError;

    const NAME: &'static str = "GetDeploymentConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DEPLOYMENT_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_DEPLOYMENT_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The CATALOG first, before any state is touched: the member is documented as
        // a fixed value, so a name outside the set of catalogs that exist names no
        // catalog to read a configuration inside.
        if !crate::state::is_supported_catalog(&input.catalog) {
            return Err(GetDeploymentConfigurationErrorModeled::ValidationException(
                crate::state::unsupported_catalog(),
            )
            .into());
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetDeploymentConfigurationErrorModeled::InternalServerException(
                crate::state::store_unavailable(),
            )
            .into());
        };

        let mut store = state.lock();
        // ONE clock sample, taken once the store is held: a job whose `SUCCEEDED`
        // status is due publishes its configuration here, so this call reports the
        // configuration that is active at that instant.
        let now = ::std::time::SystemTime::now();
        store.settle_due_jobs(now);

        if !store.holds_product(&input.catalog, &input.product_id) {
            return Err(GetDeploymentConfigurationErrorModeled::ResourceNotFoundException(
                crate::state::target_not_found("ProductId"),
            )
            .into());
        }

        let key = (
            input.catalog.clone(),
            input.product_id.clone(),
            input.delivery_option_type.clone(),
        );
        let Some(active) = store.active_configuration(&key) else {
            return Err(GetDeploymentConfigurationErrorModeled::ResourceNotFoundException(
                crate::state::target_not_found("DeliveryOptionType"),
            )
            .into());
        };

        Ok(GetDeploymentConfigurationOutput {
            configuration: active.configuration.clone(),
        })
    }
}
