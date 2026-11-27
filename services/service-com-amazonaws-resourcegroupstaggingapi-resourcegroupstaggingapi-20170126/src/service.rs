// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself. EDITED HERE: a
//!   request whose own framing contradicts itself is refused rather than served
//!   ([`contradictory_framing`]), and every other request is served by the operation the
//!   generated table names for it, wrapped as [`submitted_collections`] says.
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

use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The FRAMING one request arrives in — one of the two the model declares, the `awsJson1_1`
/// one and the Smithy RPC v2 CBOR one — and the one question this service asks about it; see
/// the module's own header.
mod request_framing;

/// What this endpoint answers a request whose framing CONTRADICTS ITSELF — one naming the
/// RPC v2 CBOR framing that also carries a header that framing prohibits; see the module's
/// own header.
mod contradictory_framing;

/// What this service states about the half of a constraint refusal the framework cannot
/// state — the COLLECTION a clause about a collection member is about; see the module's own
/// header.
mod submitted_collections;

/// The `com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry. It carries every protocol the model declares and selects one per
    /// request from the request's own headers and path, so each request is read and answered
    /// in the framing it names.
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served so that a
    /// constraint clause about a collection member states the collection the request
    /// carried ([`submitted_collections`]). Keyed by the routing key the protocol resolves
    /// (the key the generated table uses) and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `self`.
    quoting_submitted_collections: OperationRoutingTable,
    /// The answer a request whose framing contradicts itself earns, ahead of every modelled
    /// operation ([`contradictory_framing`]). Stateless, and held here rather than built per
    /// request, because [`Service::override_operation`] hands back a borrow of `self`.
    refusing_contradictory_framing: contradictory_framing::MalformedHttpRequest,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            quoting_submitted_collections:
                submitted_collections::quoting_submitted_collections(&engine),
            refusing_contradictory_framing: contradictory_framing::MalformedHttpRequest,
            entry: crate::ServiceDefinition::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, every protocol the
        // model declares, the routing table, and the default error renderers —
        // lives in the generated entry, which is read through this one accessor
        // and never edited.
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
        // NOT a re-route: the operation served is the one the generated routing
        // table names for this request, wrapped so that a constraint clause about a
        // collection member states the collection the request carried (see
        // [`submitted_collections`]). The routing DECISION stays the model's — the
        // key is resolved by the protocol of the FRAMING this request named, the one the
        // generated table is read with too — and a key this table
        // does not hold declines here, leaving the generated table and the router's
        // routing-error path unchanged. The wrapper declares the wrapped
        // operation's own auth schemes, so nothing about signing changes.
        // Before any of that: a request whose framing CONTRADICTS ITSELF — one naming the
        // RPC v2 CBOR framing that also carries a header that framing prohibits — is a
        // malformed HTTP request, refused ahead of every modelled operation and without
        // reading its body (see [`contradictory_framing`]).
        if contradictory_framing::frames_itself_contradictorily(req) {
            return Some(&self.refusing_contradictory_framing as &dyn ErasedOperation);
        }
        let operation = self.entry.wire_protocol(req).protocol().routing_key(req).ok()?;
        self.quoting_submitted_collections.get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
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
