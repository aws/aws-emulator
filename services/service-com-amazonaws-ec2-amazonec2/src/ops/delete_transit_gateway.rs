// SPDX-License-Identifier: CC0-1.0

//! @op DeleteTransitGateway
//!
//! Deletes one transit gateway, so no later reader of the store reports a gateway — or a route
//! table of one — the account no longer holds: `DescribeTransitGateways` stops answering with
//! it, whether it is asked by id or by filter.
//!
//! "You must first delete all transit gateway attachments" is the pre-condition: an attachment
//! is a resource the CALLER takes away
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::transit_gateway_attachments_of`]), so a
//! gateway that still holds one is refused for the state those attachments leave it in and
//! nothing is removed. The gateway's own route tables are NOT a dependency: they came up with
//! it, so [`crate::state::ComAmazonawsEc2Amazonec2State::remove_transit_gateway`] takes them
//! with it when the removal the delete recorded falls due
//! ([`crate::state::ComAmazonawsEc2Amazonec2State::begin_transit_gateway_deletion`]).
//!
//! `DryRun` is answered BEFORE either question: the flag asks what the CALLER may do, so the
//! permission answer precedes the existence and dependency ones, and a dry run naming a
//! gateway the account no longer holds reads the verdict rather than a not-found
//! ([`crate::state::dry_run_operation`]).
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store is
//! reached through [`crate::state::require_service_state`], the way every handler beside this
//! one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DeleteTransitGateway`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Deletes the specified transit gateway.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — Checks whether you have the required permissions for the action,
//!   without actually making the request, and provides an error response. If you have the
//!   required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `TransitGatewayId` (TransitGatewayId) **required** — The ID of the transit gateway.
//!
//! ## Output
//!
//! * `TransitGateway` (TransitGateway) — Information about the deleted transit gateway.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DeleteTransitGatewayError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DeleteTransitGatewayError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#DeleteTransitGateway")]
#[derive(Debug)]
pub enum DeleteTransitGatewayErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::delete_transit_gateway::DeleteTransitGatewayError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DeleteTransitGatewayError = crate::scaffolded::Scaffolded<DeleteTransitGatewayErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DeleteTransitGatewayErrorModeled> for DeleteTransitGatewayError {
    fn from(modeled: DeleteTransitGatewayErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DeleteTransitGateway` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DeleteTransitGateway<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DeleteTransitGateway<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DeleteTransitGateway<Eng>
{
    type Input = DeleteTransitGatewayInput;
    type Output = DeleteTransitGatewayOutput;
    type Error = DeleteTransitGatewayError;

    const NAME: &'static str = "DeleteTransitGateway";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TRANSIT_GATEWAY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DELETE_TRANSIT_GATEWAY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; this operation reads none of it — a transit
        // gateway is named by id and carries its own ARN's Region. The store is reached through
        // the engine held on `self`.
        let _ = ctx;

        let transit_gateway_id = input.transit_gateway_id.as_str();

        // The flag is the one member read before anything is resolved: "checks whether you
        // have the required permissions for the action, WITHOUT ACTUALLY MAKING THE REQUEST
        // … If you have the required permissions, the error response is DryRunOperation"
        // (`$DryRun`). That is a question about the CALLER, not about the gateway the request
        // names, so the permission answer precedes the existence one — the recorded contract
        // for this operation answers `DryRunOperation` for a dry run naming a gateway the
        // account no longer holds — and nothing is removed either way.
        if input.dry_run.unwrap_or(false) {
            return Err(state::dry_run_operation());
        }

        let state_handle = state::require_service_state(&*self.engine)?;
        let mut store = state_handle.lock();
        // The clock first, the way every other state-touching handler in this crate starts: a
        // gateway whose `available` move has fallen due is already in that state, and so is an
        // attachment of it, so every question below is asked of the state the store is ACTUALLY
        // in.
        store.materialize_due_transitions();

        // "Deletes the specified transit gateway": an id this account holds no gateway for is a
        // refusal, not a no-op — a caller that deletes twice has to be able to tell the
        // deletion it performed from one it did not.
        if !store.transit_gateways.contains_key(transit_gateway_id) {
            return Err(state::invalid_transit_gateway_id_not_found(
                transit_gateway_id,
            ));
        }

        // An attachment is a resource the CALLER takes away first — "you must first delete all
        // transit gateway attachments configured prior to" acting on the gateway
        // (`ModifyTransitGatewayOptions$AmazonSideAsn` publishes that sentence for the same
        // gateway) — so a gateway that still carries one cannot be deleted, and nothing is
        // removed. EC2 refuses it for the STATE the gateway's attachments leave it in, naming
        // the gateway THE REQUEST NAMED and no dependent id: which attachment holds it is not
        // part of the sentence AWS builds for this class.
        let attachments = store
            .transit_gateway_attachments_of(transit_gateway_id)
            .len();
        if attachments > 0 {
            return Err(state::incorrect_state(format!(
                "{transit_gateway_id} has non-deleted Transit Gateway Attachments and cannot \
                 be deleted."
            )));
        }

        let Some(deleted) = store.begin_transit_gateway_deletion(transit_gateway_id) else {
            // Unreachable: the gateway was resolved above and the store is locked throughout.
            // Kept so a delete that moved nothing can never read as a 200 over a gateway the
            // store still holds as it was.
            return Err(state::internal_error(
                "the transit gateway checked a moment ago could not be deleted",
            ));
        };
        // "Information about the deleted transit gateway" (`$TransitGateway`) as the STORE now
        // holds it: `deleting`, the state the `TransitGatewayState` catalog gives a gateway a
        // delete has been asked for, with the clock owing it the removal — so the state this
        // answer carries is one a read straight after this call corroborates.
        Ok(DeleteTransitGatewayOutput {
            transit_gateway: Some(deleted),
        })
    }
}
