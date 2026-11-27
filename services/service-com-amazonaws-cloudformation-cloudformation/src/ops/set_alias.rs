// SPDX-License-Identifier: CC0-1.0

//! @op SetAlias
//! IMPLEMENTED: puts the alias a caller asks for on the stack it names, so both the stack's
//! own name and that alias address the one stack from then on — "this enables you to
//! effectively rename a stack without recreating it". Nothing else about the stack moves:
//! "setting an alias does not change the stack's ARN, physical resource IDs, or any deployed
//! resources", so the alias is stored beside the name it stands in for
//! ([`crate::state::StoredStack::alias`]) and resolved by the one lookup every operation that
//! accepts a `StackName` goes through ([`crate::state::aliases`]).
//!
//! The two modelled refusals are the two rules the documentation states for it:
//!
//! * `AlreadyExistsException` — "the name must be unique within the same account and Region.
//!   It cannot match an existing stack name or another active alias": a Region's stack names
//!   and its active aliases are ONE namespace
//!   ([`crate::state::ComAmazonawsCloudformationCloudformationState::stack_name_taken`]), so
//!   an alias that a live stack's name or another stack's alias already holds is the resource
//!   whose "name requested already exists".
//! * `LimitExceededException` — "each stack can have at most one alias. To change a stack's
//!   alias, first remove the existing alias using RemoveAlias, then set a new one": a second
//!   alias for a stack that has one is a quota that "has already been reached", and the alias
//!   in force is left exactly as it was rather than replaced
//!   ([`crate::state::aliases::MAX_ALIASES_PER_STACK`]).
//!
//! A stack this Region holds none of is neither of those — the model declares no error for
//! it — so it is answered as the `ValidationError` this service answers a stack that is not
//! addressable with.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `SetAlias::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `SetAlias`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Assigns an alias to an existing stack. An alias is an alternative name for a stack that
//! can be used in place of the stack name in any CloudFormation API call that accepts the
//! StackName parameter. This enables you to effectively rename a stack without recreating
//! it. After setting an alias, both the original stack name and the alias resolve to the
//! same stack. Setting an alias does not change the stack's ARN, physical resource IDs, or
//! any deployed resources. Each stack can have at most one alias. To change a stack's
//! alias, first remove the existing alias using RemoveAlias, then set a new one.
//!
//! ## Input
//!
//! * `AliasName` (AliasName, len 1..128) **required** — The alias to assign to the stack. The
//!   alias can then be used in place of the stack name in any API call that accepts
//!   StackName. The name must be unique within the same account and Region. It cannot match
//!   an existing stack name or another active alias. An alias name can contain only
//!   alphanumeric characters (case-sensitive) and hyphens. It must start with an alphabetic
//!   character and can't be longer than 128 characters.
//! * `StackName` (StackName) **required** — The name or unique stack ID of the stack to
//!   assign an alias to.
//!
//! ## Errors
//!
//! * `AlreadyExistsException` (HTTP 400) — The resource with the name requested already
//!   exists.
//! * `LimitExceededException` (HTTP 400) — The quota for the resource has already been
//!   reached. For information about resource and stack limitations, see CloudFormation quotas
//!   in the CloudFormation User Guide.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SetAliasError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

#[derive(Debug, Clone, Default)]
pub struct SetAliasOutput;

impl SetAliasOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static SET_ALIAS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &SetAliasOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for SetAliasOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SetAliasError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#SetAlias")]
#[derive(Debug)]
pub enum SetAliasErrorModeled {
    AlreadyExistsException(AlreadyExistsException),
    LimitExceededException(LimitExceededException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::set_alias::SetAliasError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type SetAliasError = crate::scaffolded::Scaffolded<SetAliasErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SetAliasErrorModeled> for SetAliasError {
    fn from(modeled: SetAliasErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `SetAlias` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SetAlias<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SetAlias<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SetAlias<Eng>
{
    type Input = SetAliasInput;
    type Output = SetAliasOutput;
    type Error = SetAliasError;

    const NAME: &'static str = "SetAlias";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SET_ALIAS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = SET_ALIAS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::aliases;

        // CloudFormation checks the constraints on the stack identifier a request names
        // BEFORE it resolves the stack, so an identifier no stack could carry is answered as
        // the malformed request it is (`crate::state::stack_identifier`).
        crate::state::refuse_malformed_identifier(
            crate::state::STACK_NAME_PATH,
            Some(&input.stack_name),
        )?;
        // ...and the alias is held to its own documented rule — the same one a stack NAME is
        // held to, which is what makes the two one namespace. The model carries the length,
        // so what is left for the handler is the character rule.
        aliases::validate_alias_name(&input.alias_name)
            .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        // CloudFormation is Regional: the stack this call names an alias for is the one of
        // that name in the Region the request is signed for, and the alias is unique "within
        // the same account and Region".
        let region = crate::state::request_region(ctx.parts());

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there is no stack to assign an alias to, and
            // nothing was written.
            return Err(crate::scaffolded::Scaffolded::not_implemented("SetAlias"));
        };
        let mut store = state.lock();
        // Materialises only the transitions the clock already owed, which would have happened
        // on any other request at this instant — among them the deletion that frees a name,
        // and its alias, again.
        store.settle(::std::time::SystemTime::now());

        // "It cannot match an existing stack name or another active alias": weighed BEFORE
        // the stack is resolved, since the name a request asks for is a property of the
        // request, and so that an alias that cannot be taken is refused for the taken name
        // rather than for anything about the stack it names.
        if store.stack_name_taken(&region, &input.alias_name) {
            return Err(SetAliasErrorModeled::AlreadyExistsException(
                AlreadyExistsException {
                    message: Some(aliases::alias_already_exists(&input.alias_name)),
                },
            )
            .into());
        }

        // "Assigns an alias to an existing stack": an alias stands in for a stack name, so it
        // is set on a LIVE stack — a stack id that resolves a completed deletion names
        // nothing an alias could address.
        let stack = store
            .resolve_mut(&region, &input.stack_name)
            .filter(|stack| stack.is_live())
            .ok_or_else(|| {
                crate::scaffolded::Scaffolded::validation_error(
                    crate::state::changed_stack_does_not_exist(&input.stack_name),
                )
            })?;
        // "Each stack can have at most one alias": the stack's own quota, so a second alias
        // is refused and the one in force is left as it was.
        if stack.alias.is_some() {
            return Err(SetAliasErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(aliases::alias_limit_exceeded()),
                },
            )
            .into());
        }
        stack.alias = Some(input.alias_name.clone());

        Ok(SetAliasOutput)
    }
}
