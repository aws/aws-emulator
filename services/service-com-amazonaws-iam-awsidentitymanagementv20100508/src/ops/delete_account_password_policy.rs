// SPDX-License-Identifier: CC0-1.0

//! @op DeleteAccountPasswordPolicy
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than naming one of the conditions
//! this operation models. The input and error enum a generated stub would have carried are
//! declared in `src/ops/types/delete_account_password_policy_types.rs` and reached through
//! `super::types::*`, and the unit OUTPUT is declared below the way every rendered sibling
//! of this service with a unit output declares its own; `build.rs` picks the operation up
//! from the `@op` line above and wires it into the service's routing table, exactly as it
//! does for the hand-authored `src/ops/get_account_password_policy.rs` — the READ of this
//! same record.
//!
//! ## What this operation answers on the wire
//!
//! A MUTATION: it "deletes the password policy for the Amazon Web Services account", which
//! is the account's own single password-policy record
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::account_password_policy`])
//! — the one `UpdateAccountPasswordPolicy` writes and `GetAccountPasswordPolicy` reports:
//!
//! * a policy the account holds: it is REMOVED and the call answers `200` with an empty
//!   result, so every later reader of that record — the read of the policy, and the
//!   password checks the password-setting operations make — sees the account back on the
//!   Amazon Web Services defaults, with no custom rule left to report or enforce;
//! * no policy at all (never set, or deleted before): refused with the modelled
//!   `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`) — "the request was
//!   rejected because it referenced a resource entity that does not exist" — naming the
//!   account-level policy by the name IAM holds it under, so a repeat delete is
//!   distinguishable from the first one and nothing is removed twice.
//!
//! A caller whose identity-based policies do not permit the call is refused before anything
//! is written, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]), so a request that was not allowed deletes
//! nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `DeleteAccountPasswordPolicy`
//!
//! Deletes the password policy for the Amazon Web Services account. There are no parameters.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! None (`smithy.api#Unit`).
//!
//! ## Errors
//!
//! * `NoSuchEntityException` (HTTP 404) — The request was rejected because it referenced a
//!   resource entity that does not exist. The error message describes the resource.
//! * `LimitExceededException` (HTTP 409) — The request was rejected because it attempted to
//!   create resources beyond the current Amazon Web Services account limits.
//! * `ServiceFailureException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//!
//! For the full text of any one item, ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json")
//! ```

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteAccountPasswordPolicyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

/// The operation's output: the modelled output is `smithy.api#Unit`, so the response
/// carries the envelope and no members at all.
#[derive(Debug, Clone, Default)]
pub struct DeleteAccountPasswordPolicyOutput;

impl DeleteAccountPasswordPolicyOutput {
    /// The unit output's schema, declared the way every rendered sibling of this service
    /// with a unit output declares its own (`crate::ops::delete_account_alias`).
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static DELETE_ACCOUNT_PASSWORD_POLICY_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &DeleteAccountPasswordPolicyOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DeleteAccountPasswordPolicyOutput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#DeleteAccountPasswordPolicy")]
#[derive(Debug)]
pub enum DeleteAccountPasswordPolicyErrorModeled {
    NoSuchEntityException(NoSuchEntityException),
    LimitExceededException(LimitExceededException),
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_account_password_policy::DeleteAccountPasswordPolicyError`).
pub type DeleteAccountPasswordPolicyError =
    crate::scaffolded::Scaffolded<DeleteAccountPasswordPolicyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<DeleteAccountPasswordPolicyErrorModeled> for DeleteAccountPasswordPolicyError {
    fn from(modeled: DeleteAccountPasswordPolicyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteAccountPasswordPolicy` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DeleteAccountPasswordPolicy<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteAccountPasswordPolicy<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteAccountPasswordPolicy<Eng>
{
    type Input = DeleteAccountPasswordPolicyInput;
    type Output = DeleteAccountPasswordPolicyOutput;
    type Error = DeleteAccountPasswordPolicyError;

    const NAME: &'static str = "DeleteAccountPasswordPolicy";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DELETE_ACCOUNT_PASSWORD_POLICY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        DELETE_ACCOUNT_PASSWORD_POLICY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it removes is the account's own password
        // policy record.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                DeleteAccountPasswordPolicyErrorModeled::ServiceFailureException(
                    ServiceFailureException {
                        message: Some("the IAM store is unavailable".to_string()),
                    },
                )
                .into(),
            );
        };

        // A MUTATION: the lock is held across the decision and the write, so two concurrent
        // deletes cannot both report having removed the one record.
        let mut store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is written. The policy is the ACCOUNT's, not one
        // entity's, so the decision is made against the account-wide resource — exactly as
        // the write and the read of this same record decide.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::DELETE_ACCOUNT_PASSWORD_POLICY,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        let caller_akid = ::engine::extract_akid(&ctx.parts().headers);

        // An account holding no custom policy has none to delete, which is the one refusal
        // this operation models. Removing the record IS the delete and a record that was
        // not there IS that refusal, so taking it answers both at once: nothing is written
        // on the refused path, and on the served path the account is left holding no
        // policy at all.
        if store.account_password_policy.take().is_none() {
            ::tracing::info!(
                audit = true,
                operation = "DeleteAccountPasswordPolicy",
                outcome = "refused_no_such_entity",
                caller_akid = %caller_akid,
                "the account holds no password policy to delete"
            );
            return Err(
                DeleteAccountPasswordPolicyErrorModeled::NoSuchEntityException(
                    NoSuchEntityException {
                        message: Some(no_account_password_policy_message()),
                    },
                )
                .into(),
            );
        }

        ::tracing::info!(
            audit = true,
            operation = "DeleteAccountPasswordPolicy",
            outcome = "deleted",
            caller_akid = %caller_akid,
            "the account's password policy was deleted"
        );

        Ok(DeleteAccountPasswordPolicyOutput)
    }
}

/// How this service words `NoSuchEntityException` for a delete of a password policy the
/// account does not hold.
///
/// The `<Entity> … cannot be found.` form this service words every absent-entity request
/// with (`crate::ops::delete_account_alias` and the sentences beside it). This operation
/// addresses the ACCOUNT-level policy rather than one entity's, and the request carries no
/// value at all, so the sentence interpolates nothing: the account-level policy IAM holds
/// the password rules under has the one fixed name this names it by.
fn no_account_password_policy_message() -> String {
    format!("The account policy with name {ACCOUNT_PASSWORD_POLICY_NAME} cannot be found.")
}

/// The name IAM holds the account's password policy under, as the absent-policy refusal
/// names it. Fixed text: the account holds exactly one password policy, under this one
/// name, and no request names it.
const ACCOUNT_PASSWORD_POLICY_NAME: &str = "PasswordPolicy";
