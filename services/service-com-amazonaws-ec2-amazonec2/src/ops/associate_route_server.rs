// SPDX-License-Identifier: CC0-1.0

//! @op AssociateRouteServer
//!
//! Associates one route server with one VPC: the connection
//! `GetRouteServerAssociations` then reports, written in the non-terminal `associating`
//! state the clock owes `associated`.
//!
//! Implemented here together with its reader `GetRouteServerAssociations`, whose own stub
//! issue this change closes: without the write there is nothing for that read to report.
//! `DisassociateRouteServer` stays a stub, with its own issue.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The store
//! is reached through [`crate::state::require_service_state`], the way every handler beside
//! this one reaches it.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `AssociateRouteServer`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")
//! ```
//!
//! Associates a route server with a VPC to enable dynamic route updates. A route server
//! association is the connection established between a route server and a VPC. For more
//! information see Dynamic routing in your VPC with VPC Route Server in the Amazon VPC User
//! Guide.
//!
//! ## Input
//!
//! * `DryRun` (Boolean) — A check for whether you have the required permissions for the
//!   action without actually making the request and provides an error response. If you have
//!   the required permissions, the error response is DryRunOperation. Otherwise, it is
//!   UnauthorizedOperation.
//! * `RouteServerId` (RouteServerId) **required** — The unique identifier for the route
//!   server to be associated.
//! * `VpcId` (VpcId) **required** — The ID of the VPC to associate with the route server.
//!
//! ## Output
//!
//! * `RouteServerAssociation` (RouteServerAssociation) — Information about the association
//!   between the route server and the VPC.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `AssociateRouteServerError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ec2-amazonec2.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `AssociateRouteServerError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ec2#AssociateRouteServer")]
#[derive(Debug)]
pub enum AssociateRouteServerErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::associate_route_server::AssociateRouteServerError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type AssociateRouteServerError = crate::scaffolded::Scaffolded<AssociateRouteServerErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<AssociateRouteServerErrorModeled> for AssociateRouteServerError {
    fn from(modeled: AssociateRouteServerErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `AssociateRouteServer` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct AssociateRouteServer<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> AssociateRouteServer<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for AssociateRouteServer<Eng>
{
    type Input = AssociateRouteServerInput;
    type Output = AssociateRouteServerOutput;
    type Error = AssociateRouteServerError;

    const NAME: &'static str = "AssociateRouteServer";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_ROUTE_SERVER_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ASSOCIATE_ROUTE_SERVER_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state;

        // `ctx` carries request metadata only; every fact this association reports comes
        // from the request and from the store the engine on `self` holds.
        let _ = ctx;

        // Both ids are `@required` but modelled `clientOptional`, so a request that left
        // either out reaches this handler as an empty string rather than being refused by
        // the boundary validator. A parameter the request did not carry is EC2's
        // `MissingParameter`.
        for (parameter, value) in [
            ("routeServerId", input.route_server_id.as_str()),
            ("vpcId", input.vpc_id.as_str()),
        ] {
            if value.is_empty() {
                return Err(state::missing_parameter(format!(
                    "The request must contain the parameter {parameter}"
                )));
            }
        }

        let service_state = state::require_service_state(&*self.engine)?;
        let mut store = service_state.lock();

        // "The unique identifier for the route server to be associated" (`RouteServerId`):
        // an id that names no route server of the account is an error, not a route server
        // created out of thin air, and a route server a delete has taken down has nothing
        // left to associate. The crate's one resolver for an operation ACTING on a route
        // server tells a malformed id from an unknown one and refuses one in a delete state.
        state::require_live_route_server(&store, &input.route_server_id)?;

        // "The ID of the VPC to associate with the route server" (`VpcId`), refused with the
        // code every other operation of this service refuses an unknown VPC id with.
        if !store.vpcs.contains_key(&input.vpc_id) {
            return Err(state::invalid_vpc_id_not_found(&input.vpc_id));
        }

        // An association IS the connection between ONE route server and ONE VPC ("A route
        // server association is the connection established between a route server and a
        // VPC", `RouteServerAssociation`), and that route server then writes the routes it
        // learns into the VPC's route tables — so a route server that is already associated
        // has no second VPC to write into: a request naming it with another VPC, or naming
        // the association it already holds, is refused rather than written. A VPC, on the
        // other hand, is the SUBJECT of an association rather than its holder, so one VPC may
        // be served by more than one route server — which is what a recorded
        // `AssociateRouteServer` of a second route server onto an already-served VPC is
        // answered 200 for.
        if let Some(already_serving) =
            store.vpc_route_server_is_associated_with(&input.route_server_id)
        {
            return Err(route_server_already_associated(format!(
                "The route server '{route_server_id}' is already associated with vpc \
                 '{already_serving}'",
                route_server_id = input.route_server_id,
            )));
        }

        if !store.has_route_server_association_capacity() {
            return Err(state::route_server_limit_exceeded(format!(
                "This emulator holds {max} route server associations, which is as many as it \
                 stores",
                max = state::MAX_ROUTE_SERVER_RECORDS,
            )));
        }

        // DryRun sits AFTER every id has been resolved and every refusal asked, and BEFORE
        // the write: the flag asks whether the request WOULD have succeeded, so one that
        // would have been refused is refused with that reason instead, and one that would
        // have succeeded leaves state untouched.
        if input.dry_run == Some(true) {
            return Err(state::dry_run_operation());
        }

        // The association is written in `associating` with the clock owing it `associated`
        // one transition delay later, so a caller reading straight after this call still
        // sees `associating`. The answer is the record as it was stored, so this operation
        // and `GetRouteServerAssociations` cannot report one association differently.
        let now = ::std::time::Instant::now();
        let association =
            store.insert_route_server_association(&input.route_server_id, &input.vpc_id, now);
        Ok(AssociateRouteServerOutput {
            route_server_association: Some(association),
        })
    }
}

/// `InvalidRouteServerId.AlreadyAssociated` (HTTP 400): the route server the request named is
/// already associated, so the association it asks for would be that route server's second.
///
/// The code names the ROUTE SERVER because the route server is what an association is held
/// by: it writes the routes it learns into the route tables of the one VPC it is associated
/// with. The `message` names the pair, built from the request and from what the store already
/// holds.
fn route_server_already_associated(message: String) -> AssociateRouteServerError {
    crate::scaffolded::Scaffolded::service_error(
        "InvalidRouteServerId.AlreadyAssociated",
        400,
        message,
    )
}
