// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kafkaconnect-kafkaconnect` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself. This service
//!   uses it for the two operations whose `@required` `@httpQuery` member the REST
//!   router reads as a route discriminator (see `EmulatedService::query_keyed_operation`),
//!   so a request that leaves that member out still reaches the operation the model binds
//!   to its method and path and is refused on its own values instead of as an unknown
//!   operation.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_schema::Schema;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Operation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The operation the model's own routing table names for `req`.
fn modeled_operation<'d>(
    definition: &'d dyn ErasedServiceDefinition,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<&'d dyn ErasedOperation> {
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// The `com-amazonaws-kafkaconnect-kafkaconnect` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// `UpdateConnector`, built from the SAME shared engine the generated entry threads
    /// into every operation, so this instance and the rest of the service read one store.
    /// Held because [`Service::override_operation`] hands back a BORROW of an operation,
    /// and this is one of the two whose route the REST router keys on a `@required`
    /// `@httpQuery` member (see [`EmulatedService::query_keyed_operation`]).
    update_connector: crate::ops::update_connector::UpdateConnector<Eng>,
    /// `UntagResource`, held from that same shared engine for the other such route.
    untag_resource: crate::ops::untag_resource::UntagResource<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            update_connector: crate::ops::update_connector::UpdateConnector::new(Arc::clone(
                &engine,
            )),
            untag_resource: crate::ops::untag_resource::UntagResource::new(engine),
        }
    }

    /// The operation of this service whose own `@http` route `req` ADDRESSES, among the
    /// ones whose route the REST router keys on a `@required` `@httpQuery` member, or
    /// `None` when `req` addresses none of them.
    ///
    /// The framework's REST router treats such a member as a route DISCRIMINATOR: it
    /// rejects a candidate route whose `@required` header/query bindings are absent from
    /// the request, which is how an S3-class model tells several operations apart on ONE
    /// method + URI template by the shape of the request. This service overloads no route
    /// — every method + URI template it publishes names exactly one operation — so for a
    /// request that binds no value for such a member the rule can only misfire: the
    /// request matches NO route and is answered "unknown operation" (HTTP 501, a status no
    /// operation here models) instead of reaching the operation the model binds to its
    /// method and path, whose answer for an absent `@required` member is this service's own
    /// client-input error (HTTP 400, see [`EmulatedService::override_refusal`]).
    ///
    /// The model declares exactly two such members across this service's 21 operations —
    /// `UpdateConnector$currentVersion` and `UntagResource$tagKeys`
    /// (`smithy_query(query="operation", operation="UpdateConnector,UntagResource")`) —
    /// which is why those two operations are the ones held here; every other operation
    /// binds its `@required` members to the path or the body (or declares none), so its
    /// route always resolves and it is never reached this way. No method or URI template
    /// is written out: each candidate is matched through the shared reader
    /// (`service_common::addresses_modeled_http_route`) against the route its OWN input
    /// schema's `@http` binding gives it, so this seam cannot drift from the model, and an
    /// operation can only ever serve the route the model binds to it.
    ///
    /// What this list cannot read off the model is WHICH operations belong in it: the
    /// framework's routing table is keyed by routing key and hands back erased operations
    /// (`smithy_server_runtime::OperationRoutingTable`, whose surface is `new`/
    /// `add_operation`/`get`, and `ErasedOperation`, which exposes no input schema), so the
    /// candidates have to be named here. `tests/all/required_query_parameter_routes.rs`
    /// walks the model for that class and drives every route it finds, so a model that
    /// binds such a member on an operation this list does not hold fails that suite instead
    /// of answering a client "not implemented".
    fn query_keyed_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        let held: [(&'static Schema, &dyn ErasedOperation); 2] = [
            (
                <crate::ops::update_connector::UpdateConnector<Eng> as Operation>::INPUT_SCHEMA,
                &self.update_connector,
            ),
            (
                <crate::ops::untag_resource::UntagResource<Eng> as Operation>::INPUT_SCHEMA,
                &self.untag_resource,
            ),
        ];
        held.into_iter()
            .find(|(schema, _)| service_common::addresses_modeled_http_route(req, schema))
            .map(|(_, operation)| operation)
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // The model's own routing table is asked FIRST and is left to resolve every
        // request it can name. What it cannot name is a request that binds no value for
        // a `@required` `@httpQuery` member of the operation the model binds to its
        // method and path: the REST router reads such a member as a route
        // discriminator and matches no route at all, so the request would be answered
        // "unknown operation" (HTTP 501) although the operation is implemented and
        // models a client-input error for exactly this mistake. Such a request is
        // therefore matched against the route each candidate operation's own `@http`
        // binding gives it (`query_keyed_operation`), and the operation then answers the
        // request from the request's OWN values: the absent member is refused by the
        // framework's centralized walk, worded through `override_refusal` below, exactly
        // as an absent required PATH or BODY member of the same operation already is.
        if modeled_operation(self.definition(), req).is_some() {
            return None;
        }
        self.query_keyed_operation(req)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. KafkaConnect models its own client-input error for
        // exactly that case, so name it rather than the framework's generic
        // `ValidationException`: a client branches on the code, and ALL 21 of
        // this service's operations declare `BadRequestException` as "HTTP
        // Status Code 400: Bad request due to incorrect input. Correct your
        // request and then retry it." (checked against every `**Errors:**` line
        // in this crate's `OPERATIONS.md`, rendered from the Smithy model), so
        // the rewording names an error the model declares for whichever
        // operation the refused request was aimed at. The status the framework
        // renders for this refusal is already 400, so only the code and the
        // message change.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "BadRequestException".to_string(),
                // The one place this service words a modelled-constraint
                // refusal, shared with the one rule the boundary walk cannot
                // reach (a tag map's KEY, `crate::state::tag_key_length_refusal`),
                // so a caller cannot tell where the refusal was made.
                message: crate::state::incorrect_input_message(violations).unwrap_or_default(),
            }),
            // The framework renders an authentication refusal as 403, which is
            // this service's `ForbiddenException` ("Access forbidden. Correct
            // your credentials and then retry your request."), not its 401
            // `UnauthorizedException`.
            Refusal::Unauthenticated => Some(Refused {
                code: "ForbiddenException".to_string(),
                message: "Access forbidden. Correct your credentials and then retry your request."
                    .to_string(),
            }),
            // `Refusal` is `#[non_exhaustive]`: a refusal this service has not
            // worded keeps the framework's own wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
