// SPDX-License-Identifier: CC0-1.0

//! @op DescribeTrunkInterfaceAssociations
//! Reports the branch/trunk network-interface associations of the caller's account —
//! interface trunking, the capability EC2 grants an account out of band rather than
//! offering to every caller of this API.
//!
//! So the answer this operation gives is a fact of the ACCOUNT, not of the request: an
//! account that does not hold the capability is refused the operation itself, with HTTP 400
//! `OperationNotPermitted` naming it ([`crate::state::trunk_interface_not_permitted`]),
//! before any member of the request is interpreted — the association ids the boundary
//! validator admitted are neither resolved nor checked for shape, the filters are not
//! applied, and `MaxResults` and `NextToken` page nothing. The refusal is
//! NOT an empty page: an account outside the capability is not told that it holds no
//! associations, it is told it may not ask.
//!
//! The pair `DryRun` documents for itself — `DryRunOperation` for a caller that holds the
//! required permissions and `UnauthorizedOperation` for one that does not — is the answer to
//! the PERMISSION question, and it is answered AHEAD of the capability gate
//! ([`crate::state::dry_run_operation`]): the flag asks what the CALLER may do, which the
//! account-level capability is not, and the recorded contract for the writer of these
//! associations answers a dry run `DryRunOperation` — so all three operations of this surface
//! answer the flag the same way.
//!
//! This operation reports, so it writes nothing in any case — and there is nothing to
//! report, because the writer the associations would come from
//! (`crate::ops::associate_trunk_interface`) is gated by the same capability and stores
//! none.
//!
//! This emulator serves one account ([`::engine::emulator_account_id`]) and no operation of
//! this API grants the capability to it, so every caller reads that refusal — the same one
//! `AssociateTrunkInterface` and `DisassociateTrunkInterface` answer, from the one function
//! all three share rather than three wordings of one refusal.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. A handler
//! with state to read reaches the store through [`crate::state::require_service_state`],
//! the way every handler beside this one does; this one resolves no resource, because the
//! capability the request needs is decided before any resource is named:
//!
//! ```ignore
//!     Err(crate::state::trunk_interface_not_permitted())
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DescribeTrunkInterfaceAssociations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Describes one or more network interface trunk associations.
//!
//! ## Input
//!
//! * `AssociationIds` (TrunkInterfaceAssociationIdList) — The IDs of the associations.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `Filters` (FilterList) — One or more filters. gre-key - The ID of a trunk interface
//!   association. interface-protocol - The interface protocol. Valid values are VLAN and GRE.
//! * `MaxResults` (DescribeTrunkInterfaceAssociationsMaxResults, range 5..255) — The maximum
//!   number of results to return with a single call. To retrieve the remaining results, make
//!   another call with the returned nextToken value.
//! * `NextToken` (String) — The token for the next page of results.
//!
//! ## Output
//!
//! * `InterfaceAssociations` (TrunkInterfaceAssociationList) — Information about the trunk
//!   associations.
//! * `NextToken` (String) — The token to use to retrieve the next page of results. This value
//!   is null when there are no more results to return.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeTrunkInterfaceAssociationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeTrunkInterfaceAssociationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DescribeTrunkInterfaceAssociations")]
#[derive(Debug)]
pub enum DescribeTrunkInterfaceAssociationsErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_trunk_interface_associations::DescribeTrunkInterfaceAssociationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeTrunkInterfaceAssociationsError = crate::scaffolded::Scaffolded<DescribeTrunkInterfaceAssociationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeTrunkInterfaceAssociationsErrorModeled> for DescribeTrunkInterfaceAssociationsError {
    fn from(modeled: DescribeTrunkInterfaceAssociationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeTrunkInterfaceAssociations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeTrunkInterfaceAssociations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeTrunkInterfaceAssociations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeTrunkInterfaceAssociations<Eng>
{
    type Input = DescribeTrunkInterfaceAssociationsInput;
    type Output = DescribeTrunkInterfaceAssociationsOutput;
    type Error = DescribeTrunkInterfaceAssociationsError;

    const NAME: &'static str = "DescribeTrunkInterfaceAssociations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_TRUNK_INTERFACE_ASSOCIATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_TRUNK_INTERFACE_ASSOCIATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Neither the request nor the store decides this answer, so neither is read: the
        // capability interface trunking needs is granted to an ACCOUNT, and the account this
        // emulator serves does not hold it, so every request — whatever association ids,
        // filters or page it names, and whichever Region it was made in — is refused the
        // operation rather than told anything about what it asked for. `DryRun` is the one
        // member read, and ahead of that refusal (see the module docs). Nothing is
        // written: this operation reports, and a refused report leaves the account exactly
        // as it found it.
        let _ = (ctx, &self.engine);
        if input.dry_run == Some(true) {
            return Err(crate::state::dry_run_operation());
        }
        Err(crate::state::trunk_interface_not_permitted())
    }
}
