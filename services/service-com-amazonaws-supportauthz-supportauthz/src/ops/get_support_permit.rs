// SPDX-License-Identifier: CC0-1.0

//! @op GetSupportPermit
//!
//! Hand-authored: the scaffold op-stub generator declined this operation
//! (`src/ops/SKIPPED_STUBS.md`) because its `Permit` shape reaches two unions
//! whose "all" member targets the Smithy prelude shape `smithy.api#Unit`. This
//! file is patterned on the rendered siblings in this directory and is the ONLY
//! file to touch for this operation.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetSupportPermit`
//!
//! From this service's Smithy model. For the FULL text of any one item:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-supportauthz-supportauthz/smithy-model/com-amazonaws-supportauthz-supportauthz.json")
//! ```
//!
//! Retrieves the details of a support permit by its ARN or name.
//!
//! **Route:** `GET /support-permits/{supportPermitIdentifier}` -> 200, `@readonly`
//!
//! ## Input
//!
//! * `supportPermitIdentifier` (SupportPermitIdentifier, len 1..512, pattern
//!   ^[a-zA-Z0-9:/-]{1,512}$, @httpLabel) **required** — the permit's ARN or its name.
//!
//! ## Output
//!
//! * `name`, `arn`, `permit`, `status`, `signingKeyInfo`, `createdAt` — all
//!   **required** — plus `description`, `supportCaseDisplayId` and `tags`.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `InternalServerException` (HTTP 500),
//!   `ResourceNotFoundException` (HTTP 404), `ThrottlingException` (HTTP 429),
//!   `ValidationException` (HTTP 400).

#[allow(unused_imports)]
use super::types::*;

// The shapes of this operation the scaffold declines to render, declared from the
// model in this crate's own source; `crate::service` is where that module is
// declared, because the `@generated` `types/mod.rs` cannot keep a `mod` line.
#[allow(unused_imports)]
use crate::service::types_local::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportauthz-supportauthz.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.supportauthz#GetSupportPermit")]
#[derive(Debug)]
pub enum GetSupportPermitErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type GetSupportPermitError = crate::scaffolded::Scaffolded<GetSupportPermitErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetSupportPermitErrorModeled> for GetSupportPermitError {
    fn from(modeled: GetSupportPermitErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetSupportPermit` operation.
pub struct GetSupportPermit<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetSupportPermit<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetSupportPermit<Eng>
{
    type Input = GetSupportPermitInput;
    type Output = GetSupportPermitOutput;
    type Error = GetSupportPermitError;

    const NAME: &'static str = "GetSupportPermit";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::GET_SUPPORT_PERMIT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::GET_SUPPORT_PERMIT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Request metadata only; the store is reached through `self.engine`.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetSupportPermitErrorModeled::InternalServerException(InternalServerException {
                message: "SupportAuthZ permit state is unavailable on this engine.".to_string(),
                retry_after_seconds: None,
            })
        })?;
        let mut state = state.lock();

        // ONE instant decides both the clock's work and this read: a permit
        // whose `DeleteSupportPermit` removal was already due is gone whether or
        // not anybody called. Materialising that is the only write this
        // reporting operation makes.
        state.purge_due(::std::time::SystemTime::now());

        // The identifier is either the ARN or the name — the modeled label
        // pattern admits both, and the store indexes both.
        let Some(record) = state.resolve(&input.support_permit_identifier) else {
            return Err(
                GetSupportPermitErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: format!(
                            "No support permit matches '{}'.",
                            input.support_permit_identifier
                        ),
                        resource_id: input.support_permit_identifier.clone(),
                        resource_type: crate::state::SUPPORT_PERMIT_RESOURCE_TYPE.to_string(),
                    },
                )
                .into(),
            );
        };

        Ok(GetSupportPermitOutput {
            name: record.name.clone(),
            arn: record.arn.clone(),
            description: record.description.clone(),
            permit: record.permit.clone(),
            status: record.status.clone(),
            signing_key_info: record.signing_key_info.clone(),
            created_at: record.created_at,
            support_case_display_id: record.support_case_display_id.clone(),
            tags: Some(record.tags_map()),
        })
    }
}
