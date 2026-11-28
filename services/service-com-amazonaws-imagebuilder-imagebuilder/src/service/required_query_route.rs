// SPDX-License-Identifier: CC0-1.0

//! The operation a request ADDRESSED when the REST route lookup matched it to no operation at
//! all, only because a member that operation requires outside its body is absent.
//!
//! RULE: a request whose method and path are the ones an operation of this service binds IS
//! that operation's request. A member that operation requires and the request does not carry
//! is then a statement about the REQUEST — this service refuses it naming the properties the
//! request had to carry ([`super::parameter_refusal`]), exactly as it already refuses a
//! required member absent from a BODY — and never a report that the operation is not
//! implemented.
//!
//! WHY it has to be answered here: every `@required` `@httpQuery` / `@httpHeader` member is
//! part of the ROUTE the framework matches, so a candidate route whose required binding the
//! request does not carry is not selected at all (`route_lookup`,
//! `vendor/smithy-schema-server/src/rest_xml.rs`, which this model's `restJson1` protocol
//! routes through). That rule exists to tell several operations apart on ONE method + URI
//! template by the shape of the request; it picks BETWEEN operations and cannot un-name the
//! only operation a route has. With no route selected the request falls to the
//! unknown-operation tail, which for a REST envelope is HTTP 501 `NotImplemented` — the claim
//! that nobody has written the operation yet ([`crate::scaffolded`], whose own documentation
//! says that claim must not answer a bad request), and a class an SDK retry policy keeps
//! retrying.
//!
//! Naming the operation from [`smithy_server_runtime::Service::override_operation`] hands such
//! a request to the operation it addressed, so the centralized request walk's own boundary
//! refuses it: the walk reports the member the request left out and this service's front door
//! words it (HTTP 400 `InvalidRequestException`, "Your request is invalid. Request must
//! contain ['<member>'] properties. Verify your request and try again."). The handler never
//! runs, so a refused request writes nothing.
//!
//! NOTHING about a route is written down here, and no operation is named: each route is read
//! off the operation's OWN generated input schema — which carries its `@http` method and URI
//! pattern — by the shared reader every service with such a route uses
//! ([`::service_common::addresses_modeled_http_route`]), and every operation this service
//! serves registers its own ([`ModelledRoutes::serving`]). So this layer covers each
//! operation of the model that requires a member outside its body without a list of them to
//! keep in step, and it can never name an operation the service does not serve.

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema::Schema;
use smithy_server_runtime::{ErasedOperation, Operation, OperationRoutingTable};

/// The `@http` route of every modelled operation this service serves: the routing key the
/// operation is served under, and its own generated input schema, which carries the method and
/// the URI pattern that route is read from.
#[derive(Debug, Default)]
pub struct ModelledRoutes {
    routes: Vec<(&'static str, &'static Schema)>,
}

impl ModelledRoutes {
    /// An empty set of routes, filled as each operation is served.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `O` is being served, so a request that addresses its modelled route can be
    /// handed to it when the framework's own lookup withholds that route.
    pub fn serving<O: Operation>(&mut self) {
        self.routes.push((O::NAME, O::INPUT_SCHEMA));
    }

    /// The operation in `table` whose modelled route `req` addresses, when EXACTLY one
    /// operation of this service binds that method and path — and `None` for every other
    /// request, which keeps the answer it already had.
    ///
    /// Asked from [`smithy_server_runtime::Service::override_operation`] only once the
    /// protocol's own routing key has named no operation, so nothing that routes today routes
    /// differently: this reads only the requests that would otherwise be answered as naming no
    /// operation at all.
    ///
    /// Exactly one, because a method + URI template several operations share is the case the
    /// framework's own lookup exists to decide between: deciding it from the method and the
    /// path alone would pick an operation the request may not have addressed, so such a
    /// request keeps the framework's answer.
    pub fn addressed<'table>(
        &self,
        req: &RtRequest,
        table: &'table OperationRoutingTable,
    ) -> Option<&'table dyn ErasedOperation> {
        let mut addressed = self
            .routes
            .iter()
            .filter(|(_, schema)| ::service_common::addresses_modeled_http_route(req, schema));
        let (operation, _) = addressed.next()?;
        if addressed.next().is_some() {
            return None;
        }
        table.get(operation)
    }
}
