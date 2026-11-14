// SPDX-License-Identifier: CC0-1.0

//! @op AssociateTrunkInterface
//! Joins a `branch` network interface to a `trunk` one — interface trunking, the capability
//! EC2 grants an account out of band rather than offering to every caller of this API.
//!
//! So the answer this operation gives is a fact of the ACCOUNT, not of the request: an
//! account that does not hold the capability is refused the operation itself, with HTTP 400
//! `OperationNotPermitted` naming it ([`crate::state::trunk_interface_not_permitted`]),
//! before any member of the request is interpreted — the interface ids the boundary
//! validator admitted are neither resolved nor checked for shape, the VLAN id and the GRE
//! key are not read, and a `ClientToken` records nothing. Nothing is stored:
//! there is no association to store, and a refused call leaves every interface of the
//! account exactly as it found it.
//!
//! `DryRun` is answered AHEAD of that refusal, with the permitted side of the probe
//! ([`crate::state::dry_run_operation`]): "checks whether you have the required permissions
//! for the action, without actually making the request" (`$DryRun`) asks about the CALLER's
//! permission to perform the action, which the account-level capability above is not — and
//! the recorded contract for this operation answers a dry run `DryRunOperation`.
//!
//! This emulator serves one account ([`::engine::emulator_account_id`]) and no operation of
//! this API grants the capability to it, so every caller reads that refusal. The reader
//! `DescribeTrunkInterfaceAssociations` and `DisassociateTrunkInterface` are gated by the
//! same capability and answer the shared refusal above rather than a second wording of it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. A handler
//! with state to read or write reaches the store through
//! [`crate::state::require_service_state`], the way every handler beside this one does;
//! this one reads no resource and writes none, because the capability the request needs is
//! decided before any resource is named:
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
//! # Modeled contract for `AssociateTrunkInterface`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Associates a branch network interface with a trunk network interface. Before you create
//! the association, use CreateNetworkInterface command and set the interface type to trunk.
//! You must also create a network interface for each branch network interface that you want
//! to associate with the trunk network interface.
//!
//! ## Input
//!
//! * `BranchInterfaceId` (NetworkInterfaceId) **required** — The ID of the branch network
//!   interface.
//! * `ClientToken` (String, @idempotencyToken) — Unique, case-sensitive identifier that you
//!   provide to ensure the idempotency of the request. For more information, see Ensuring
//!   idempotency.
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `GreKey` (Integer) — The application key. This applies to the GRE protocol.
//! * `TrunkInterfaceId` (NetworkInterfaceId) **required** — The ID of the trunk network
//!   interface.
//! * `VlanId` (Integer) — The ID of the VLAN. This applies to the VLAN protocol.
//!
//! ## Output
//!
//! * `ClientToken` (String) — Unique, case-sensitive identifier that you provide to ensure
//!   the idempotency of the request. For more information, see Ensuring idempotency.
//! * `InterfaceAssociation` (TrunkInterfaceAssociation) — Information about the association
//!   between the trunk network interface and branch network interface.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateTrunkInterfaceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateTrunkInterfaceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#AssociateTrunkInterface")]
#[derive(Debug)]
pub enum AssociateTrunkInterfaceErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_trunk_interface::AssociateTrunkInterfaceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateTrunkInterfaceError = crate::scaffolded::Scaffolded<AssociateTrunkInterfaceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateTrunkInterfaceErrorModeled> for AssociateTrunkInterfaceError {
    fn from(modeled: AssociateTrunkInterfaceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateTrunkInterface` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateTrunkInterface<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateTrunkInterface<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateTrunkInterface<Eng>
{
    type Input = AssociateTrunkInterfaceInput;
    type Output = AssociateTrunkInterfaceOutput;
    type Error = AssociateTrunkInterfaceError;

    const NAME: &'static str = "AssociateTrunkInterface";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_TRUNK_INTERFACE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_TRUNK_INTERFACE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Neither the request nor the store decides this answer, so neither is read: the
        // capability interface trunking needs is granted to an ACCOUNT, and the account this
        // emulator serves does not hold it, so every request — whatever interfaces, VLAN id,
        // GRE key or client token it carries, and whichever Region it was made in — is
        // refused the operation rather than told anything about what it named. `DryRun` is
        // the one member read, and ahead of that refusal: the flag asks whether the CALLER
        // holds the action's permissions (see the module docs).
        let _ = (ctx, &self.engine);
        if input.dry_run == Some(true) {
            return Err(crate::state::dry_run_operation());
        }
        Err(crate::state::trunk_interface_not_permitted())
    }
}
