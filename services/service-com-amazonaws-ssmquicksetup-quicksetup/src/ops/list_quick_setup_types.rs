// SPDX-License-Identifier: CC0-1.0

//! @op ListQuickSetupTypes
//!
//! Reports this service's whole Quick Setup type catalog: one
//! `QuickSetupTypeOutput` per configuration type the service offers, carrying that
//! type's name and the latest configuration version available for it — the newest
//! version of the service-owned template that type deploys, computed from that type's
//! own published version history, so two types whose templates have advanced
//! differently report different versions. It is the discovery call a client makes
//! before authoring a `ConfigurationDefinition`, whose `Type` and `TypeVersion` are
//! chosen from this catalog.
//!
//! `@readonly`, and the catalog is service data rather than per-account state, so this
//! handler takes no lock and writes nothing: the answer is the same for every caller,
//! in every region, whatever the store holds. The catalog itself lives in
//! [`crate::state::QUICK_SETUP_TYPE_CATALOG`], which documents where its membership
//! comes from.
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
//! Reach the store through [`crate::state::service_state`]; never construct an engine
//! in a handler. This operation needs no store, so it touches neither.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListQuickSetupTypes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json")
//! ```
//!
//! Returns the available Quick Setup types.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `QuickSetupTypeList` (QuickSetupTypeList) — An array of Quick Setup types.
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
// `ListQuickSetupTypesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.ssmquicksetup#ListQuickSetupTypes")]
#[derive(Debug)]
pub enum ListQuickSetupTypesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_quick_setup_types::ListQuickSetupTypesError`).
pub type ListQuickSetupTypesError = crate::scaffolded::Scaffolded<ListQuickSetupTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the
/// handler writes `Err(SomeModeledError.into())` rather than naming the wrapper
/// variant.
impl From<ListQuickSetupTypesErrorModeled> for ListQuickSetupTypesError {
    fn from(modeled: ListQuickSetupTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListQuickSetupTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct ListQuickSetupTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListQuickSetupTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListQuickSetupTypes<Eng>
{
    type Input = ListQuickSetupTypesInput;
    type Output = ListQuickSetupTypesOutput;
    type Error = ListQuickSetupTypesError;

    const NAME: &'static str = "ListQuickSetupTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_QUICK_SETUP_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_QUICK_SETUP_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it, and the
        // catalog is the same for every authorized caller: no request metadata and no
        // store entry takes part in the answer.
        let _ = &input;

        // WHO is asking does take part: AWS decides a signed call at the IAM layer, so
        // the caller's own identity-based policies are evaluated before the catalog is
        // reported. This operation addresses no resource of its own, so it is judged
        // against IAM's own spelling of "no particular resource"
        // (`crate::state::identity_authorization::ANY_RESOURCE`).
        if let Some(denied) = crate::state::identity_authorization::access_denied(
            &*self.engine,
            ctx,
            Self::NAME,
            crate::state::identity_authorization::ANY_RESOURCE,
        ) {
            return Err(ListQuickSetupTypesErrorModeled::AccessDeniedException(denied).into());
        }

        Ok(ListQuickSetupTypesOutput {
            quick_setup_type_list: Some(
                crate::state::QUICK_SETUP_TYPE_CATALOG
                    .iter()
                    .map(|entry| QuickSetupTypeOutput {
                        r#type: Some(entry.type_.to_string()),
                        // The newest version this type's published history holds, not
                        // one version the whole catalog shares: each type's template
                        // is versioned independently.
                        latest_version: Some(entry.latest_version().to_string()),
                    })
                    .collect(),
            ),
        })
    }
}
