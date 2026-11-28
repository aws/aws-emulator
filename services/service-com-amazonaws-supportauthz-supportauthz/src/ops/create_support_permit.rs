// SPDX-License-Identifier: CC0-1.0

//! @op CreateSupportPermit
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
//! Reach the store through [`crate::state::service_state`] and read and write it
//! through the guard it hands back. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `CreateSupportPermit`
//!
//! From this service's Smithy model. For the FULL text of any one item:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-supportauthz-supportauthz/smithy-model/com-amazonaws-supportauthz-supportauthz.json")
//! ```
//!
//! Creates a support permit that authorizes an AWS support operator to perform specified
//! actions on specified resources.
//!
//! **Route:** `POST /support-permits` -> 200
//!
//! ## Input
//!
//! * `name` (Name, len 1..256, pattern ^[a-zA-Z0-9]{1,256}$) **required**
//! * `permit` (Permit) **required** — the authorized actions, resources and conditions.
//! * `signingKeyInfo` (SigningKeyInfo union: `kmsKey`) **required**
//! * `description` (Description, len 1..1024)
//! * `supportCaseDisplayId` (SupportCaseDisplayId, len 1..512, pattern
//!   ^[a-zA-Z0-9:/-]{1,512}$)
//! * `clientToken` (ClientToken, len 1..128, pattern ^[!-~]+$, @idempotencyToken)
//! * `tags` (Tags, map<string,string>, len 0..50)
//!
//! ## Output
//!
//! * `name`, `arn` (Arn, pattern ^arn:[a-z0-9-]+:[a-z0-9-]+:[a-z0-9-]*:[0-9]{12}:.+$),
//!   `permit`, `status` (SupportPermitStatus: ACTIVE | DELETING | INACTIVE),
//!   `signingKeyInfo`, `createdAt` (timestamp) — all **required** — plus
//!   `description`, `supportCaseDisplayId` and `tags`.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `ConflictException` (HTTP 409),
//!   `InternalServerException` (HTTP 500), `ServiceQuotaExceededException` (HTTP 402),
//!   `ThrottlingException` (HTTP 429), `ValidationException` (HTTP 400).

#[allow(unused_imports)]
use super::types::*;

// The shapes of this operation the scaffold declines to render, declared from the
// model in this crate's own source; `crate::service` is where that module is
// declared, because the `@generated` `types/mod.rs` cannot keep a `mod` line.
#[allow(unused_imports)]
use crate::service::types_local::*;

use std::collections::BTreeMap;
use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateSupportPermitError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportauthz-supportauthz.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.supportauthz#CreateSupportPermit")]
#[derive(Debug)]
pub enum CreateSupportPermitErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type CreateSupportPermitError = crate::scaffolded::Scaffolded<CreateSupportPermitErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CreateSupportPermitErrorModeled> for CreateSupportPermitError {
    fn from(modeled: CreateSupportPermitErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateSupportPermit` operation.
pub struct CreateSupportPermit<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateSupportPermit<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateSupportPermit<Eng>
{
    type Input = CreateSupportPermitInput;
    type Output = CreateSupportPermitOutput;
    type Error = CreateSupportPermitError;

    const NAME: &'static str = "CreateSupportPermit";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::CREATE_SUPPORT_PERMIT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::CREATE_SUPPORT_PERMIT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateSupportPermitErrorModeled::InternalServerException(InternalServerException {
                message: "SupportAuthZ permit state is unavailable on this engine.".to_string(),
                retry_after_seconds: None,
            })
        })?;
        let mut state = state.lock();

        // ONE instant decides both the clock's work and this call: a permit
        // whose `DeleteSupportPermit` removal was already due is gone whether or
        // not anybody called, so its name is free for this create.
        state.purge_due(SystemTime::now());

        let tags: BTreeMap<String, String> = input
            .tags
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let fingerprint = crate::state::create_fingerprint(
            &input.name,
            input.description.as_deref(),
            &input.permit,
            &input.signing_key_info,
            input.support_case_display_id.as_deref(),
            &tags,
        );

        // `@idempotencyToken` replay. A token already used for THIS body answers
        // with the permit it created — no second permit, and no conflict. The
        // same token with any other body is a conflict: one token names one
        // create.
        if let Some(token) = input.client_token.as_deref() {
            if let Some(replay) = state.create_replay(token) {
                let replayed_arn = replay.arn.clone();
                if replay.fingerprint != fingerprint {
                    return Err(CreateSupportPermitErrorModeled::ConflictException(
                        ConflictException {
                            message: format!(
                                "The clientToken '{token}' was already used to create a different \
                                 support permit."
                            ),
                            resource_id: replayed_arn,
                            resource_type: crate::state::SUPPORT_PERMIT_RESOURCE_TYPE.to_string(),
                        },
                    )
                    .into());
                }
                if let Some(record) = state.get(&replayed_arn) {
                    return Ok(output_for(record));
                }
            }
        }

        // One name holds one permit: a second create under a live name is a
        // conflict, not a silent replacement of the first permit's authorization.
        if let Some(existing) = state.by_name(&input.name) {
            let existing_arn = existing.arn.clone();
            return Err(
                CreateSupportPermitErrorModeled::ConflictException(ConflictException {
                    message: format!(
                        "A support permit named '{}' already exists.",
                        input.name
                    ),
                    resource_id: existing_arn,
                    resource_type: crate::state::SUPPORT_PERMIT_RESOURCE_TYPE.to_string(),
                })
                .into(),
            );
        }

        // The store holds a bounded number of permits, so a caller cannot grow
        // it without limit. The operation's own `ServiceQuotaExceededException`
        // is exactly this answer, and the cap bounds every map on the state
        // struct (an idempotency entry and a page cursor are dropped with the
        // permit they name).
        if state.len() >= crate::state::MAX_SUPPORT_PERMITS {
            return Err(
                CreateSupportPermitErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: format!(
                            "The support-permit quota of {} is already in use; delete a support \
                             permit before creating another.",
                            crate::state::MAX_SUPPORT_PERMITS
                        ),
                        resource_id: input.name.clone(),
                        resource_type: crate::state::SUPPORT_PERMIT_RESOURCE_TYPE.to_string(),
                        service_code: crate::state::SERVICE_CODE.to_string(),
                        quota_code: crate::state::SUPPORT_PERMIT_QUOTA_CODE.to_string(),
                    },
                )
                .into(),
            );
        }

        // The ARN is minted through the core's `emulator_arn`, so the account
        // segment is the canonical emulator account and the whole string
        // satisfies the modeled `Arn` pattern.
        let arn = crate::state::support_permit_arn(&crate::state::request_region(ctx), &input.name);
        let created_at = crate::state::now();

        let record = crate::state::SupportPermitRecord {
            name: input.name.clone(),
            arn: arn.clone(),
            description: input.description.clone(),
            permit: input.permit.clone(),
            // A permit AWS returns from a create is usable at once: `ACTIVE` is
            // terminal, so no later transition is owed.
            status: crate::state::STATUS_ACTIVE.to_string(),
            signing_key_info: input.signing_key_info.clone(),
            created_at,
            support_case_display_id: input.support_case_display_id.clone(),
            tags,
            removal_due_at: None,
        };

        state.insert(record);
        if let Some(token) = input.client_token.as_deref() {
            state.remember_create(token, &arn, fingerprint);
        }

        // Answered from the STORE, not from `input`: what the next reader sees is
        // exactly what this call reports.
        let stored = state.get(&arn).ok_or_else(|| {
            CreateSupportPermitErrorModeled::InternalServerException(InternalServerException {
                message: "The support permit could not be read back after it was stored."
                    .to_string(),
                retry_after_seconds: None,
            })
        })?;
        Ok(output_for(stored))
    }
}

/// The create response for a stored permit.
fn output_for(record: &crate::state::SupportPermitRecord) -> CreateSupportPermitOutput {
    CreateSupportPermitOutput {
        name: record.name.clone(),
        arn: record.arn.clone(),
        description: record.description.clone(),
        permit: record.permit.clone(),
        status: record.status.clone(),
        signing_key_info: record.signing_key_info.clone(),
        created_at: record.created_at,
        support_case_display_id: record.support_case_display_id.clone(),
        tags: Some(record.tags_map()),
    }
}
