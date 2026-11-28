// SPDX-License-Identifier: CC0-1.0

//! @op DeleteSupportPermit
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
//! # Modeled contract for `DeleteSupportPermit`
//!
//! From this service's Smithy model. For the FULL text of any one item:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-supportauthz-supportauthz/smithy-model/com-amazonaws-supportauthz-supportauthz.json")
//! ```
//!
//! Deletes a support permit, revoking the authorization previously granted to the AWS
//! support operator.
//!
//! **Route:** `DELETE /support-permits/{supportPermitIdentifier}` -> 200, `@idempotent`
//!
//! ## Input
//!
//! * `supportPermitIdentifier` (Arn, len 1..512, pattern
//!   ^arn:[a-z0-9-]+:[a-z0-9-]+:[a-z0-9-]*:[0-9]{12}:.+$, @httpLabel) **required** — the
//!   permit's ARN. Unlike `GetSupportPermit`'s identifier, this one is modeled as an
//!   `Arn`, so a bare name is refused by the boundary validator before `handle` runs.
//!
//! ## Output
//!
//! * `name`, `arn`, `permit`, `status`, `signingKeyInfo`, `createdAt` — all
//!   **required** — plus `description` and `supportCaseDisplayId`. The delete output
//!   shape carries no `tags`, and documents `status` as "Returns DELETING".
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
#[smithy_macros::operation_error("com.amazonaws.supportauthz#DeleteSupportPermit")]
#[derive(Debug)]
pub enum DeleteSupportPermitErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type DeleteSupportPermitError = crate::scaffolded::Scaffolded<DeleteSupportPermitErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DeleteSupportPermitErrorModeled> for DeleteSupportPermitError {
    fn from(modeled: DeleteSupportPermitErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteSupportPermit` operation.
pub struct DeleteSupportPermit<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteSupportPermit<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteSupportPermit<Eng>
{
    type Input = DeleteSupportPermitInput;
    type Output = DeleteSupportPermitOutput;
    type Error = DeleteSupportPermitError;

    const NAME: &'static str = "DeleteSupportPermit";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::DELETE_SUPPORT_PERMIT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::DELETE_SUPPORT_PERMIT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Request metadata only; the store is reached through `self.engine`.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            DeleteSupportPermitErrorModeled::InternalServerException(InternalServerException {
                message: "SupportAuthZ permit state is unavailable on this engine.".to_string(),
                retry_after_seconds: None,
            })
        })?;
        let mut state = state.lock();

        // ONE instant decides both the clock's work and this call: a permit
        // whose removal was already due is gone whether or not anybody called,
        // so deleting it again is a 404 rather than a second acceptance.
        let now = ::std::time::SystemTime::now();
        state.purge_due(now);

        // `@idempotent`: accepting the deletion moves the permit to the
        // NON-terminal `DELETING` — which is what this operation's `status`
        // member documents ("Returns DELETING") — and sets its removal due a
        // fixed span later. A second delete inside that span answers with the
        // same record and does not push the due instant out. Until the removal
        // falls due, a caller still observes the permit in `DELETING` through
        // `GetSupportPermit` and the `supportPermitStatuses=DELETING` filter;
        // after it, `GetSupportPermit` answers `ResourceNotFoundException` and
        // `ListSupportPermits` no longer carries it.
        let Some(record) = state.begin_delete(&input.support_permit_identifier, now) else {
            return Err(
                DeleteSupportPermitErrorModeled::ResourceNotFoundException(
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

        Ok(DeleteSupportPermitOutput {
            name: record.name,
            arn: record.arn,
            description: record.description,
            permit: record.permit,
            status: record.status,
            signing_key_info: record.signing_key_info,
            created_at: record.created_at,
            support_case_display_id: record.support_case_display_id,
        })
    }
}
