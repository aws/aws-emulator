// SPDX-License-Identifier: CC0-1.0

//! @op GetScope
//! Reads one scope back out of [`crate::state`]: every member comes from the record
//! `CreateScope` persisted, so a read answers exactly what the create answered. REPORTING
//! only — nothing is written here.
//!
//! HAND-AUTHORED, not generated, for the reason `ops/create_scope.rs` records: the
//! scaffold's op-stub generator declined every scope operation ("shape missing from model:
//! smithy.api#Unit", see `src/ops/SKIPPED_STUBS.md`). Its shapes live in
//! `types/scope_types.rs`.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetScope::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. Only the `handle` body is this file's.
//!
//! ---
//!
//! # Modeled contract for `GetScope`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="GetScope",model_path="services/service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json")`).
//!
//! Retrieves a scope. `GET /scopes/{scopeIdentifier}`, success status 200, `@readonly`.
//!
//! ## Input
//!
//! * `scopeIdentifier` (ScopeIdentifier, len 1..1010, @httpLabel) **required** — The
//!   identifier of the scope: its Amazon Resource Name (ARN).
//!
//! ## Output
//!
//! The same member set `CreateScope` answers with: `scopeId`, `scopeArn`, `scopeName`,
//! `status` and `version` **required**, plus `scopeDescription`, `scopeConfiguration`,
//! `specializedScopeType`, `updateToken`, `isSnapshot`, `hasPublishedVersion` and
//! `updatedAt`.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permissions to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — The request processing failed because of an
//!   internal error in the service. This is a retryable error.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — The request was denied because of request
//!   throttling.
//! * `ValidationException` (HTTP 400) — The request failed validation.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.networksecuritymanager#GetScope")]
#[derive(Debug)]
pub enum GetScopeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_scope::GetScopeError`).
pub type GetScopeError = crate::scaffolded::Scaffolded<GetScopeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetScopeErrorModeled> for GetScopeError {
    fn from(modeled: GetScopeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetScope` operation.
pub struct GetScope<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetScope<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetScope<Eng>
{
    type Input = GetScopeInput;
    type Output = GetScopeOutput;
    type Error = GetScopeError;

    const NAME: &'static str = "GetScope";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_SCOPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_SCOPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetScopeErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the scope store is unavailable".to_string(),
                },
            )
            .into());
        };

        let store = state.lock();
        let Some(scope) = store.scope_by_identifier(&input.scope_identifier) else {
            // The miss names the resource type this read resolves and the scope id the
            // identifier carries — the service's wording for an absent entity, stated
            // once in `crate::state`.
            return Err(GetScopeErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(
                    crate::state::RESOURCE_TYPE_SCOPE,
                    &input.scope_identifier,
                ),
            )
            .into());
        };

        Ok(GetScopeOutput {
            scope_id: scope.scope_id.clone(),
            scope_arn: scope.scope_arn.clone(),
            scope_name: scope.scope_name.clone(),
            scope_description: scope.scope_description.clone(),
            scope_configuration: Some(scope.scope_configuration.clone()),
            specialized_scope_type: scope.specialized_scope_type.clone(),
            status: scope.status.clone(),
            version: scope.version_string(),
            update_token: Some(scope.update_token.clone()),
            is_snapshot: Some(scope.is_snapshot),
            has_published_version: Some(scope.has_published_version),
            updated_at: Some(scope.updated_at_timestamp()),
        })
    }
}
