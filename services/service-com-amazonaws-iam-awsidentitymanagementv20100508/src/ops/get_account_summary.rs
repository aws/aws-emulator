// SPDX-License-Identifier: CC0-1.0

//! @op GetAccountSummary
//!
//! HAND-AUTHORED: the scaffold op-stub generator declined to emit a stub for this
//! operation because its input is `smithy.api#Unit`, which it does not type yet (see
//! `src/ops/SKIPPED_STUBS.md`), so the operation was left modeled-but-unwired and the
//! framework answered it from the unroutable-operation path — an awsQuery `InvalidAction`,
//! which tells a client the API does not exist rather than reporting the account. The
//! input, output, error enum and schemas a generated stub would have carried are declared
//! in `src/ops/types/get_account_summary_types.rs` (the layout every other operation of
//! this service follows) and reached through `super::types::*`; `build.rs` picks the
//! operation up from the `@op` line above and wires it into the service's routing table.
//!
//! ## What this operation answers on the wire
//!
//! A REPORT: it "retrieves information about IAM entity usage and IAM quotas in the Amazon
//! Web Services account", so every entry of the `SummaryMap` it answers is read off the
//! account's own records and nothing is written — two calls with no write between them
//! report the same pairs. The counts and the quotas themselves are
//! [`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::account_summary`],
//! beside the state they aggregate, so the quota this reports and the quota the operation
//! enforcing it refuses past are one constant.
//!
//! A caller whose identity-based policies do not permit the call is refused before
//! anything is reported, `403` `<Code>AccessDenied</Code>`
//! ([`crate::state::authorization::authorize`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `GetAccountSummary`
//!
//! Retrieves information about IAM entity usage and IAM quotas in the Amazon Web Services
//! account. For information about IAM quotas, see IAM and STS quotas in the IAM User Guide.
//!
//! ## Input
//!
//! None (`smithy.api#Unit`).
//!
//! ## Output
//!
//! * `SummaryMap` (summaryMapType) — A set of key–value pairs containing information about
//!   IAM entity usage and IAM quotas. The keys are the values `summaryKeyType` declares and
//!   the values are integers (`summaryValueType`).
//!
//! ## Errors
//!
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
// `GetAccountSummaryError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.iam#GetAccountSummary")]
#[derive(Debug)]
pub enum GetAccountSummaryErrorModeled {
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_account_summary::GetAccountSummaryError`).
pub type GetAccountSummaryError = crate::scaffolded::Scaffolded<GetAccountSummaryErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetAccountSummaryErrorModeled> for GetAccountSummaryError {
    fn from(modeled: GetAccountSummaryErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetAccountSummary` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetAccountSummary<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetAccountSummary<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetAccountSummary<Eng>
{
    type Input = GetAccountSummaryInput;
    type Output = GetAccountSummaryOutput;
    type Error = GetAccountSummaryError;

    const NAME: &'static str = "GetAccountSummary";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_SUMMARY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_ACCOUNT_SUMMARY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The operation takes no input: what it answers comes from the account's own
        // records and from the quotas this crate states.
        let _ = &input;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetAccountSummaryErrorModeled::ServiceFailureException(
                ServiceFailureException {
                    message: Some("the IAM store is unavailable".to_string()),
                },
            )
            .into());
        };
        let store = state.lock();

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is reported. The summary is the ACCOUNT's, not one
        // entity's, and the Service Authorization Reference lists no resource type for the
        // action, so the decision is made against the account-wide resource — exactly as
        // the other account-wide reports of this service decide.
        crate::state::authorization::authorize(
            &store,
            ctx,
            crate::state::authorization::GET_ACCOUNT_SUMMARY,
            crate::state::authorization::ACCOUNT_WIDE_RESOURCE,
        )?;

        // The modelled member is a map, so what is reported is the SET of pairs the
        // account's usage and quotas make up; every one of them is aggregated from the
        // store in the one place that aggregation lives.
        let summary_map: ::std::collections::HashMap<String, i32> =
            store.account_summary().into_iter().collect();
        Ok(GetAccountSummaryOutput {
            summary_map: Some(summary_map),
        })
    }
}
