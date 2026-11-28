// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-connecthealth-connecthealth` service: how it claims requests and how it words the
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
//!   generated routing table. Return `Some(op)` to route one yourself.
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
//!
//! One of those seams is taken here: `override_operation` routes the ONE operation the
//! generated table cannot serve at all, `StartMedicalScribeListeningSession` — its output
//! is a `@streaming` union, so the generated shim declines the request and this file hands
//! it to the operation's own event-stream implementation instead. It also names the one
//! class of request the generated table resolves NOTHING for although the model binds its
//! method and path to an operation: one that left a `@required` query-bound member out, so
//! the REST route lookup withheld the route ([`required_query_route`]). Every other request
//! goes through the generated routing table untouched, so the status, the error code and
//! the body of a refusal are the framework's own restJson1 rendering of the modelled shape
//! (the detail text it carries is built in [`error_detail`]).
//!
//! A second seam is taken: `override_refusal` words the ONE framework-level refusal this
//! service does not state as the framework does — a `domainId` outside what `DomainId`
//! declares, which it refuses with its own domain-id sentence ([`domain_id_refusal`]).

pub mod domain_id_refusal;
pub mod error_detail;
pub mod required_query_route;
pub mod tag_bounds;

/// The exception shapes of this service and the WIRE KEY each plane of it carries their
/// detail text under ([`exception_wire`]) — the other half of how this service words a
/// refusal, beside the sentences [`error_detail`] builds.
///
/// Declared HERE, from this never-regenerated file, although the source sits beside the
/// generated shapes it replaces: `src/ops/types/mod.rs` is rendered from the model by the
/// scaffold generator and rewritten by every model update, so a declaration added there
/// does not survive one. The `#[path]` is what keeps the file where the module it belongs
/// to can still find it while the declaration lives where it cannot be overwritten.
#[path = "ops/types/exception_wire.rs"]
pub mod exception_wire;

use crate::ops::start_medical_scribe_listening_session::{
    self, StartMedicalScribeListeningSession,
};
use service_common::event_stream::EventStreamRoute;
use service_common::request_path;
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-connecthealth-connecthealth` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The one event-stream operation this service serves. It is in the generated
    /// routing table too, but not in a servable form: the generated `ErasedOperation`
    /// shim of an operation whose output is a `@streaming` union declines the request
    /// (it cannot drive `serialize_as_event_stream<P>` through an object-safe `run`), so
    /// this copy is held behind an [`EventStreamRoute`] and routed by
    /// [`EmulatedService::override_operation`] — which the router consults FIRST, before
    /// the generated table. Both copies read the same domains, subscriptions and
    /// sessions, because that state lives in the shared engine rather than on the
    /// operation.
    start_medical_scribe_listening_session: EventStreamRoute<StartMedicalScribeListeningSession<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            start_medical_scribe_listening_session: EventStreamRoute(
                StartMedicalScribeListeningSession::new(engine),
            ),
        }
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
        // ONE operation is routed ahead of the table: `StartMedicalScribeListeningSession`,
        // whose output is a `@streaming` union and whose generated shim therefore
        // declines the request. It is matched by the method and the path the model binds
        // it to, through the operation's own consts, so this cannot drift from the model.
        if req.method() == start_medical_scribe_listening_session::MODELED_METHOD
            && request_path(req.uri()) == start_medical_scribe_listening_session::MODELED_PATH
        {
            return Some(&self.start_medical_scribe_listening_session);
        }
        // A SECOND class is named here, and only this class: a request the model's own
        // routing key resolves nothing for, which nevertheless addressed the method and
        // path of an operation of this service and was matched to no route only because a
        // `@required` query-bound member of that route is absent. Handing it to the
        // operation it addressed is what lets the centralized walk refuse it as the bad
        // request it is — this service's `ValidationException` (400) naming the absent
        // member — instead of the protocol's routing default claiming, with 501, that
        // nobody has implemented the operation (see [`required_query_route`]).
        if let Some(addressed) = required_query_route::addressed_operation(&self.entry, req) {
            return Some(addressed);
        }
        // Every other request resolves through the generated routing table.
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. ONE refusal is reworded: a value supplied for this service's
        // `domainId` member that is outside what `DomainId` declares, which ConnectHealth
        // states as its own domain-id sentence rather than as the framework's generic
        // member-constraint clause (see [`domain_id_refusal`]). The code it carries is
        // unchanged, so a client branching on the code sees the same answer; every other
        // refusal keeps the framework's wording.
        domain_id_refusal::refused(refusal)
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
