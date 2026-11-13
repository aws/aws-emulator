// SPDX-License-Identifier: CC0-1.0

//! @op GetServiceSettings
//!
//! Reports the Quick Setup settings of the account and region this request was
//! signed for — what the last successful `UpdateServiceSettings` wrote there.
//!
//! Settings are not a creatable resource: the operation models no way for them to be
//! absent (its errors are AccessDenied/Conflict/InternalServer/Throttling, with no
//! not-found and no validation error), so every account and region always has a
//! settings record and every call reports the `ServiceSettings` structure. A region
//! no update has configured is a 200 carrying that structure with its optional
//! `ExplorerEnablingRoleArn` absent — the unconfigured state lives INSIDE the
//! structure — never a 404, never a 501 and never a body without the member.
//!
//! `@readonly`: this handler only reads the store. It never writes, so a Get leaves
//! the next caller seeing exactly what it saw; reporting the structure for a region
//! nothing was written for persists nothing.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this
//! operation because its input is `smithy.api#Unit` (recorded in
//! `src/ops/SKIPPED_STUBS.md`), so it had no handler file and no entry in the
//! service's `operations(...)` allow-list and was answered by the framework's
//! unroutable-operation path — an HTTP 501 that is not one of the operation's modelled
//! outcomes. `build.rs` derives that allow-list from the `//! @op <Name>` header of
//! every `src/ops/*.rs`, so this file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`] and read it through the
//! guard it hands back; never construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! the SigV4 credential scope this handler reads the region from lives there.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetServiceSettings`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json")
//! ```
//!
//! Returns settings configured for Quick Setup in the requesting Amazon Web Services
//! account and Amazon Web Services Region.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `ServiceSettings` (ServiceSettings) — Returns details about the settings for Quick
//!   Setup in the requesting Amazon Web Services account and Amazon Web Services Region.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The requester has insufficient permissions to
//!   perform the operation.
//! * `ConflictException` (HTTP 409) — Another request is being processed. Wait a few minutes
//!   and try again.
//! * `InternalServerException` (HTTP 500) — An error occurred on the server side.
//! * `ThrottlingException` (HTTP 429) — The request or operation exceeds the maximum allowed
//!   request rate per Amazon Web Services account and Amazon Web Services Region.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetServiceSettingsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.ssmquicksetup#GetServiceSettings")]
#[derive(Debug)]
pub enum GetServiceSettingsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_service_settings::GetServiceSettingsError`).
pub type GetServiceSettingsError = crate::scaffolded::Scaffolded<GetServiceSettingsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<GetServiceSettingsErrorModeled> for GetServiceSettingsError {
    fn from(modeled: GetServiceSettingsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetServiceSettings` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetServiceSettings<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetServiceSettings<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetServiceSettings<Eng>
{
    type Input = GetServiceSettingsInput;
    type Output = GetServiceSettingsOutput;
    type Error = GetServiceSettingsError;

    const NAME: &'static str = "GetServiceSettings";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SERVICE_SETTINGS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SERVICE_SETTINGS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it: which
        // settings record this reports is decided by the region the request was signed
        // for, exactly as the region the writer keyed it under was.
        let _ = &input;

        // The caller's own identity-based policies are evaluated before the settings are
        // read. This operation addresses no resource of its own, so it is judged against
        // IAM's own spelling of "no particular resource"
        // (`crate::state::identity_authorization::ANY_RESOURCE`).
        if let Some(denied) = crate::state::identity_authorization::access_denied(
            &*self.engine,
            ctx,
            Self::NAME,
            crate::state::identity_authorization::ANY_RESOURCE,
        ) {
            return Err(GetServiceSettingsErrorModeled::AccessDeniedException(denied).into());
        }

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetServiceSettingsErrorModeled::InternalServerException(InternalServerException {
                message: Some("the Quick Setup service settings store is unavailable".to_string()),
            })
        })?;

        let region = crate::state::request_region(ctx.parts());

        // Read-only: take the guard, copy what this region's settings are, release it.
        // No branch of this handler writes. What a region no update has configured
        // answers is decided in one place, `service_settings_in`, which this delegates
        // to rather than restating: every region HAS settings, so the answer is always
        // the structure, carrying only the members an update wrote.
        let service_settings = state.lock().service_settings_in(&region);

        Ok(GetServiceSettingsOutput {
            service_settings: Some(service_settings),
        })
    }
}
