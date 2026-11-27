// SPDX-License-Identifier: CC0-1.0

//! @op RemoveAlias
//! IMPLEMENTED: takes the alias a caller names off the stack carrying it, so "after the alias
//! is removed successfully, it can no longer be used to reference the stack in API calls" —
//! the stack goes on being addressable by its own name, and the alias name is free for another
//! stack ([`crate::state::aliases`]). This is the other half of `SetAlias`: "to change a
//! stack's alias, first remove the existing alias using RemoveAlias, then set a new one".
//!
//! The model declares no error for this operation, so an alias this account and Region holds
//! none of is answered as the `ValidationError` this service answers an address that resolves
//! nothing with — never as a removal that succeeded without removing anything.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `RemoveAlias::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`; the
//! Region a request is served in comes from there ([`crate::state::request_region`]).
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `RemoveAlias`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Removes an alias from a stack. After the alias is removed successfully, it can no longer
//! be used to reference the stack in API calls.
//!
//! ## Input
//!
//! * `AliasName` (AliasName, len 1..128) **required** — The alias name to remove.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `RemoveAliasError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

#[derive(Debug, Clone, Default)]
pub struct RemoveAliasOutput;

impl RemoveAliasOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static REMOVE_ALIAS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &RemoveAliasOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for RemoveAliasOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `RemoveAliasError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#RemoveAlias")]
#[derive(Debug)]
pub enum RemoveAliasErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::remove_alias::RemoveAliasError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type RemoveAliasError = crate::scaffolded::Scaffolded<RemoveAliasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<RemoveAliasErrorModeled> for RemoveAliasError {
    fn from(modeled: RemoveAliasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `RemoveAlias` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct RemoveAlias<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> RemoveAlias<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for RemoveAlias<Eng>
{
    type Input = RemoveAliasInput;
    type Output = RemoveAliasOutput;
    type Error = RemoveAliasError;

    const NAME: &'static str = "RemoveAlias";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::REMOVE_ALIAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = REMOVE_ALIAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::aliases;

        // An alias is held to the rule its name is set under, checked on the INPUT and ahead
        // of the lookup: a value no alias could ever carry is a malformed request rather than
        // an alias this Region does not hold.
        aliases::validate_alias_name(&input.alias_name)
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        // An alias is unique "within the same account and Region", so the one this call
        // removes is the one of that name in the Region the request is signed for.
        let region = crate::state::request_region(ctx.parts());

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no stack holding an alias, and
            // nothing was written.
            return Err(crate::scaffolded::Scaffolded::not_implemented("RemoveAlias"));
        };
        let mut store = state.lock();
        // Materialises only the transitions the clock already owed, which would have happened
        // on any other request at this instant — among them the deletion that takes a stack,
        // and the alias addressing it, out of reach.
        store.settle(::std::time::SystemTime::now());

        // The alias addresses the stack that carries it, so the removal is a write on THAT
        // stack: an alias no live stack of this Region carries resolves nothing to remove.
        let Some(stack_id) = store
            .stack_with_alias(&region, &input.alias_name)
            .map(|stack| stack.stack_id.clone())
        else {
            return Err(crate::scaffolded::Scaffolded::validation_error(
                aliases::alias_does_not_exist(&input.alias_name),
            ));
        };
        if let Some(stack) = store.resolve_mut(&region, &stack_id) {
            // "After the alias is removed successfully, it can no longer be used to reference
            // the stack": the name goes off the stack, which is what frees it for another
            // stack and what stops it resolving this one.
            stack.alias = None;
        }

        Ok(RemoveAliasOutput)
    }
}
