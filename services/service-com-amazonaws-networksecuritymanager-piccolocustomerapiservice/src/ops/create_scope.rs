// SPDX-License-Identifier: CC0-1.0

//! @op CreateScope
//! Mints and PERSISTS a scope in [`crate::state`]: every member of the response is read
//! back out of the stored record, so a following `GetScope` observes exactly what this call
//! answered.
//!
//! HAND-AUTHORED, not generated: the scaffold's op-stub generator declined this operation
//! ("shape missing from model: smithy.api#Unit", see `src/ops/SKIPPED_STUBS.md`), so it was
//! not in the service's `operations(..)` allow-list at all — and a route the model binds
//! but the service does not wire is answered with a protocol-level 501, which no SDK models
//! as an error of this service. Its shapes live in `types/scope_types.rs`.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateScope::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. Only the `handle` body is this file's.
//!
//! Every modelled input constraint — `scopeName`'s `@pattern` and `@length`,
//! `clientToken`'s, `resourceScopes`' key set — is enforced by that walk BEFORE `handle`
//! runs, and refused with HTTP 400 `ValidationException`. Reaching `handle` means the input
//! satisfies all of them.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach the
//! store through [`crate::state::service_state`] and read and write it through the guard it
//! hands back. `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata
//! only — the engine is NOT reached through `ctx`.
//!
//! ---
//!
//! # Modeled contract for `CreateScope`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="CreateScope",model_path="services/service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json")`).
//!
//! Creates a scope. `POST /scopes`, success status 201.
//!
//! ## Input
//!
//! * `clientToken` (IdempotencyToken, len 1..64, pattern ^[\x21-\x7E]+$, @idempotencyToken)
//!   — A unique, case-sensitive token that you provide to ensure that the operation
//!   completes no more than one time.
//! * `scopeName` (ScopeName, len 1..128, pattern ^[a-zA-Z0-9][a-zA-Z0-9 _.:/=+\-@]*$)
//!   **required** — The name of the scope.
//! * `scopeDescription` (Description, len ..256, pattern ^[a-zA-Z0-9 _.:/=+\-@]*$) — A
//!   description of the scope.
//! * `scopeConfiguration` (ScopeConfiguration) **required** — The accounts and resource
//!   types the scope selects.
//! * `specializedScopeType` (SpecializedScopeType) — Omit this value to create a standard
//!   scope that you can associate with a deployment. Set it to create a scope that is built
//!   for one specific purpose.
//! * `isPublished` (IsPublished, default true) **required** — When true, the resource is
//!   saved in published (ACTIVE) state. When false, it is saved as a draft (DRAFT).
//! * `tags` (TagMap, len ..200) — The tags to add to the resource when it is created.
//!
//! ## Output
//!
//! * `scopeId` (ScopeId, len 1..64, pattern ^[a-z0-9]{1,64}$) **required**
//! * `scopeArn` (ScopeArn, len 20..1010, ARN pattern) **required**
//! * `scopeName` (ScopeName) **required**
//! * `scopeDescription` (Description)
//! * `scopeConfiguration` (ScopeConfiguration)
//! * `specializedScopeType` (SpecializedScopeType) — the scope's specialized type, if it
//!   has one; set at create and never changed
//! * `status` (EntityStatus, one of: ACTIVE | DISABLED | DRAFT) **required**
//! * `version` (EntityVersion, len 1..10, pattern ^[1-9][0-9]*$) **required**
//! * `updateToken` (UpdateToken, len 36..36, UUID pattern)
//! * `isSnapshot` (IsSnapshot), `hasPublishedVersion` (HasPublishedVersion)
//! * `updatedAt` (DateTimestamp, @timestampFormat date-time)
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permissions to perform
//!   this action.
//! * `ConflictException` (HTTP 409) — The request conflicts with the current state of the
//!   resource.
//! * `InternalServerException` (HTTP 500) — The request processing failed because of an
//!   internal error in the service. This is a retryable error.
//! * `ServiceQuotaExceededException` (HTTP 402) — The request would exceed a service quota.
//! * `ServiceUnavailableException` (HTTP 503) — The service is temporarily unavailable.
//! * `TagPolicyViolationException` (HTTP 400) — The request violates a tag policy.
//! * `ThrottlingException` (HTTP 429) — The request was denied because of request
//!   throttling.
//! * `ValidationException` (HTTP 400) — The request failed validation. For details, see the
//!   reason and fieldList members of the response.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateScopeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-networksecuritymanager-piccolocustomerapiservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.networksecuritymanager#CreateScope")]
#[derive(Debug)]
pub enum CreateScopeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ServiceUnavailableException(ServiceUnavailableException),
    TagPolicyViolationException(TagPolicyViolationException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_scope::CreateScopeError`).
pub type CreateScopeError = crate::scaffolded::Scaffolded<CreateScopeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so this handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<CreateScopeErrorModeled> for CreateScopeError {
    fn from(modeled: CreateScopeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateScope` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct CreateScope<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateScope<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateScope<Eng>
{
    type Input = CreateScopeInput;
    type Output = CreateScopeOutput;
    type Error = CreateScopeError;

    const NAME: &'static str = "CreateScope";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_SCOPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CREATE_SCOPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 201u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(CreateScopeErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the scope store is unavailable".to_string(),
                },
            )
            .into());
        };

        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let idempotency_key = input
            .client_token
            .as_deref()
            .map(|token| crate::state::idempotency_key(Self::NAME, token));

        let mut store = state.lock();

        // `clientToken` is `@idempotencyToken`: "If you retry a request with the same
        // client token and the same parameters, the service returns the result of the
        // original successful request." The original result is replayed out of the store;
        // this service models no parameter-mismatch error, so a replay never becomes a
        // failure and never mints a second scope.
        if let Some(existing) = idempotency_key
            .as_deref()
            .and_then(|key| store.idempotent_call(key))
            .map(|call| call.resource_id.clone())
            .and_then(|scope_id| store.scopes.get(&scope_id))
        {
            return Ok(output_of(existing));
        }

        // A scope's name is NOT one of its identifiers — the model's `scopeIdentifier` is
        // the scope's ARN — and no modelled member or error makes it unique, so two scopes
        // may carry the same `scopeName` and stay distinct resources, exactly as two rules
        // may. Nothing is refused here for a name already in use.

        // The store is bounded: past its capacity a create is REFUSED with the modelled
        // quota error rather than growing the process for as long as callers keep asking.
        if store.scopes.len() >= crate::state::MAX_SCOPES {
            return Err(CreateScopeErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: format!(
                        "this account already holds the maximum of {} scopes; delete one \
                         before creating another",
                        crate::state::MAX_SCOPES
                    ),
                    resource_type: Some(crate::state::RESOURCE_TYPE_SCOPE.to_string()),
                    ..ServiceQuotaExceededException::default()
                },
            )
            .into());
        }

        let scope_id = store.mint_scope_id();
        let scope = crate::state::Scope {
            scope_arn: ::engine::emulator_arn(
                crate::state::ARN_NAMESPACE,
                &region,
                &format!("scope/{scope_id}"),
            ),
            scope_id: scope_id.clone(),
            scope_name: input.scope_name,
            scope_description: input.scope_description,
            scope_configuration: input.scope_configuration,
            // "This value is set when the scope is created and can't be changed": stored
            // as submitted, and `None` for a standard scope.
            specialized_scope_type: input.specialized_scope_type,
            status: crate::state::status_for(input.is_published).to_string(),
            // `EntityVersion` starts at 1; `UpdateScope` is what bumps it.
            version: 1,
            update_token: crate::state::mint_update_token(),
            // What this operation creates is the live resource, not a snapshot of a
            // published version (`CreateScopeSnapshot` makes those).
            is_snapshot: false,
            has_published_version: input.is_published,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            created_at: now,
            updated_at: now,
        };

        // Rendered from the record that is about to be stored, so the response cannot
        // disagree with what a later read returns.
        let output = output_of(&scope);
        if let Some(key) = idempotency_key {
            store.remember_idempotent_call(key, scope_id.clone(), scope.version);
        }
        store.scopes.insert(scope_id, scope);
        Ok(output)
    }
}

/// The `CreateScopeOutput` view of a persisted scope.
fn output_of(scope: &crate::state::Scope) -> CreateScopeOutput {
    CreateScopeOutput {
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
    }
}
