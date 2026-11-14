// SPDX-License-Identifier: CC0-1.0

//! @op ExportAutomatedReasoningPolicyVersion
//!
//! Reports the policy definition held by the REVISION a `policyArn` names, read out of
//! [`crate::state`]: the working draft for an unversioned ARN, and the frozen snapshot a
//! version cut for an ARN carrying the `:<version>` suffix the shape admits. An ARN this
//! account holds no policy under is answered with 404 `ResourceNotFoundException`, and so
//! is one naming a version the policy has not cut — reported as the missing version rather
//! than as a missing policy ([`crate::state::AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND`]),
//! exactly as a read of that ARN is.
//!
//! The exported definition is the revision's CONTENT as
//! [`crate::state::policy_definition_content`] reads it — the format version this service
//! writes definitions in ([`crate::state::POLICY_DEFINITION_FORMAT_VERSION`]), and each of
//! its three lists, an absent one as the empty list — which is the same reading
//! [`crate::state::policy_definition_hash`] digests. So a
//! revision that holds no definition exports the empty definition rather than refusing a
//! read of a policy that is there, and the definition this operation exports always hashes
//! to the `definitionHash` the same revision reports.
//!
//! A pure READ (`@readonly`): nothing about a policy or one of its versions is owed a
//! change by the clock, so this writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ExportAutomatedReasoningPolicyVersion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Exports the policy definition for an Automated Reasoning policy version. Returns the
//! complete policy definition including rules, variables, and custom variable types in a
//! structured format.
//!
//! ## Input
//!
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   **required** — The Amazon Resource Name (ARN) of the Automated Reasoning policy to
//!   export. Can be either the unversioned ARN for the draft policy or a versioned ARN for a
//!   specific policy version.
//!
//! ## Output
//!
//! * `policyDefinition` (AutomatedReasoningPolicyDefinition) **required** — The exported
//!   policy definition containing the formal logic rules, variables, and custom variable
//!   types.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ExportAutomatedReasoningPolicyVersionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ExportAutomatedReasoningPolicyVersionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ExportAutomatedReasoningPolicyVersion")]
#[derive(Debug)]
pub enum ExportAutomatedReasoningPolicyVersionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::export_automated_reasoning_policy_version::ExportAutomatedReasoningPolicyVersionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ExportAutomatedReasoningPolicyVersionError = crate::scaffolded::Scaffolded<ExportAutomatedReasoningPolicyVersionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ExportAutomatedReasoningPolicyVersionErrorModeled> for ExportAutomatedReasoningPolicyVersionError {
    fn from(modeled: ExportAutomatedReasoningPolicyVersionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ExportAutomatedReasoningPolicyVersion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ExportAutomatedReasoningPolicyVersion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ExportAutomatedReasoningPolicyVersion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ExportAutomatedReasoningPolicyVersion<Eng>
{
    type Input = ExportAutomatedReasoningPolicyVersionInput;
    type Output = ExportAutomatedReasoningPolicyVersionOutput;
    type Error = ExportAutomatedReasoningPolicyVersionError;

    const NAME: &'static str = "ExportAutomatedReasoningPolicyVersion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXPORT_AUTOMATED_REASONING_POLICY_VERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::EXPORT_AUTOMATED_REASONING_POLICY_VERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ExportAutomatedReasoningPolicyVersionErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "automated reasoning policy state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let store = state.lock();

        // The ARN is matched as the caller spelled it, so an ARN of another region names no
        // policy this account holds; the `:<version>` suffix the shape admits selects a
        // version this policy has cut, and the unversioned ARN its working draft.
        let Some((policy, version)) = store.automated_reasoning_policy_revision(&input.policy_arn)
        else {
            // One unresolvable ARN, one sentence: whether it reports the missing version or
            // the missing policy is the store's own rule, which every read addressed by this
            // ARN shares.
            return Err(
                ExportAutomatedReasoningPolicyVersionErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(
                            store.automated_reasoning_policy_revision_not_found_message(
                                &input.policy_arn,
                            ),
                        ),
                    },
                )
                .into(),
            );
        };

        // A numbered version exports the definition it FROZE, which a later write to the
        // draft cannot move; the unversioned ARN exports the draft as it stands now.
        let definition = match version {
            Some(version) => version.definition.as_ref(),
            None => policy.definition.as_ref(),
        };
        // The revision's definition as its CONTENT, which is the same reading the
        // `definitionHash` beside it is taken over: a revision holding no definition exports
        // the empty definition rather than refusing a read of a policy that is there.
        Ok(ExportAutomatedReasoningPolicyVersionOutput {
            policy_definition: crate::state::policy_definition_content(definition),
        })
    }
}
