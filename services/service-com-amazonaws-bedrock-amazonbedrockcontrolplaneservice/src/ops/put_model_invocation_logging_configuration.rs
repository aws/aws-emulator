// SPDX-License-Identifier: CC0-1.0

//! @op PutModelInvocationLoggingConfiguration
//!
//! Stores the `loggingConfig` the request carries as the account's model invocation
//! logging configuration in
//! [`crate::state::ComAmazonawsBedrockAmazonbedrockcontrolplaneserviceState::model_invocation_logging`],
//! so `GetModelInvocationLoggingConfiguration` reports it back.
//!
//! The configuration is a SETTINGS record, and this call is `@idempotent`: it
//! REPLACES whatever was in force whole rather than merging into it, so a member the
//! request omits is not configured afterwards even if an earlier put set it, and the
//! same request applied twice leaves the same configuration behind. The response has
//! no members.
//!
//! Every constraint the model states — `logGroupName` 1..512, the `roleArn` IAM role
//! pattern, `bucketName` 3..63, `keyPrefix` ..1024 — is refused by the centralized
//! validation walk BEFORE this handler runs, so a request that reaches here carries a
//! well-formed configuration.
//!
//! A well-formed configuration is still not an installable one. A destination has to RESOLVE
//! where the service that owns it holds it: the CloudWatch Logs destination names the group
//! the records are written into and the role they are written under, and Amazon Bedrock
//! validates the delivery permissions that pair describes before it enables the destination,
//! so a group CloudWatch Logs reports it does not hold or a role this account's IAM reports
//! it holds nothing under is refused with the modelled `ValidationException` (HTTP 400)
//! stating the submitted pair back, with nothing stored
//! ([`crate::state::unvalidated_cloud_watch_destination`]).
//!
//! A destination that names a role the CALLER supplied can only be installed by passing
//! Amazon Bedrock that role, and [`crate::state::pass_role_denial`] takes that decision —
//! for the CloudWatch Logs
//! destination, the one whose `roleArn` is `@required` — BEFORE anything is stored. That
//! decision is taken over the SUBMITTED value: the account's pass-role boundary over that
//! role has to allow the CALLING principal the `iam:PassRole` the hand-off is, the role has
//! to be one of the CALLING account (`iam:PassRole` is granted by the account that holds the
//! role), and, where this account's IAM holds it, its trust policy has to admit the Amazon
//! Bedrock service principal the `sts:AssumeRole` such a hand-off is. A pass no part refuses
//! installs the destination, so it is read back; a denied pass is answered with the modelled
//! `AccessDeniedException` (HTTP 403) and leaves the account singleton exactly as it was,
//! so an independent `GetModelInvocationLoggingConfiguration` reports no part of the
//! refused configuration. A configuration that passes no role (`s3Config` only, or no
//! destination at all) has no such decision to take and is stored as given.
//!
//! Neither is a destination Amazon Bedrock could not deliver into. Delivery is REGIONAL —
//! the service writes the records from the Region the configuration is installed in — so a
//! bucket living in another Region is refused with the modelled `ValidationException`
//! (HTTP 400) rather than installed, and nothing is stored for it
//! ([`crate::state::cross_region_logging_bucket`], which judges both buckets a
//! configuration can name).
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PutModelInvocationLoggingConfiguration`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Set the configuration values for model invocation logging.
//!
//! ## Input
//!
//! * `loggingConfig` (LoggingConfig) **required** — The logging configuration values to set.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PutModelInvocationLoggingConfigurationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PutModelInvocationLoggingConfigurationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#PutModelInvocationLoggingConfiguration")]
#[derive(Debug)]
pub enum PutModelInvocationLoggingConfigurationErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::put_model_invocation_logging_configuration::PutModelInvocationLoggingConfigurationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PutModelInvocationLoggingConfigurationError = crate::scaffolded::Scaffolded<PutModelInvocationLoggingConfigurationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PutModelInvocationLoggingConfigurationErrorModeled> for PutModelInvocationLoggingConfigurationError {
    fn from(modeled: PutModelInvocationLoggingConfigurationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `PutModelInvocationLoggingConfiguration` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PutModelInvocationLoggingConfiguration<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PutModelInvocationLoggingConfiguration<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PutModelInvocationLoggingConfiguration<Eng>
{
    type Input = PutModelInvocationLoggingConfigurationInput;
    type Output = PutModelInvocationLoggingConfigurationOutput;
    type Error = PutModelInvocationLoggingConfigurationError;

    const NAME: &'static str = "PutModelInvocationLoggingConfiguration";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_MODEL_INVOCATION_LOGGING_CONFIGURATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUT_MODEL_INVOCATION_LOGGING_CONFIGURATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The region the request was signed for, which is where the role a destination
        // names is resolved against this account's IAM.
        let region = crate::state::request_region(ctx.parts());
        // The destination is RESOLVED first: Amazon Bedrock validates the delivery
        // permissions a CloudWatch Logs destination describes before it enables it, and
        // neither half of that pair can be validated while the service that owns it reports
        // the resource absent — so such a configuration is refused as input, with the
        // submitted pair stated back ([`crate::state::unvalidated_cloud_watch_destination`]).
        if let Some(refusal) =
            crate::state::unvalidated_cloud_watch_destination(ctx, &region, &input.logging_config)
                .await
        {
            return Err(
                PutModelInvocationLoggingConfigurationErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(refusal),
                    },
                )
                .into(),
            );
        }
        // Then the PassRole step, still ahead of the store: a destination whose role DOES
        // resolve and may not be passed to Amazon Bedrock is refused, and a refused put
        // installs nothing.
        if let Some(denial) =
            crate::state::pass_role_denial(&*self.engine, ctx, &region, &input.logging_config)
                .await
        {
            return Err(
                PutModelInvocationLoggingConfigurationErrorModeled::AccessDeniedException(
                    AccessDeniedException {
                        message: Some(denial),
                    },
                )
                .into(),
            );
        }
        // Then the object destinations: delivery is regional, so a bucket outside this
        // Region is not a destination this configuration could be served from and the
        // configuration is refused as input rather than installed
        // ([`crate::state::logging_destination`]).
        if let Some(refusal) =
            crate::state::cross_region_logging_bucket(ctx, &region, &input.logging_config).await
        {
            return Err(
                PutModelInvocationLoggingConfigurationErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(refusal),
                    },
                )
                .into(),
            );
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                PutModelInvocationLoggingConfigurationErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "model invocation logging state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        state.lock().model_invocation_logging = Some(input.logging_config);

        Ok(PutModelInvocationLoggingConfigurationOutput {})
    }
}
