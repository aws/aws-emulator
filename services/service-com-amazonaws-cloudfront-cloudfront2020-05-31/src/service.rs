// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudfront-cloudfront2020-05-31` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name. EDITED HERE: it
//!   claims the control plane as the framework's own rule does, and beside it a request
//!   addressed to a DISTRIBUTION's own domain name — the edge data plane, which no route
//!   key names (see [`edge`]).
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
//! ## The error envelope
//!
//! `override_operation` below claims every request the model's routing table resolves
//! and runs it through [`envelope::ErrorEnvelope`], which dispatches the modelled
//! operation unchanged and then finishes the response's error envelope: CloudFront's
//! `<ErrorResponse>` names the request it answers (`RequestId`), which is a property of
//! the envelope rather than a member of any error shape, and a refusal the request
//! boundary states in the framework's vocabulary is restated in this service's own
//! modelled one. That module's docs carry the rules; routing stays with the generated
//! table, which the seam reads rather than replaces.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## This service's other edited seam: the distribution edge
//!
//! CloudFront publishes TWO kinds of hostname. The CONTROL plane is
//! `cloudfront.amazonaws.com`, whose first label names the service, and that is what the
//! framework's route key resolves. The DATA plane is the per-distribution edge domain
//! `d<label>.cloudfront.net` (plus every alternate domain name the distribution's
//! `Aliases` carry), whose first label is the distribution's own generated label: it names
//! no service, so the front door can resolve no target service from such a request and
//! answers `404 UnknownService` for the hostname this service told the caller to use.
//! [`edge`] owns that plane — what it resolves, what CloudFront itself decides and what it
//! fetches from an origin — and two seams carry it:
//!
//! - [`Service::can_handle`] also claims a request addressed to a distribution this
//!   service HOLDS ([`edge::claims_parts`]), which is what routes it here at all;
//! - [`Service::override_operation`] serves it, as an [`ErasedOperation`]
//!   ([`EdgeDataPlane`]) so the handler is handed the in-process transport it fetches the
//!   origin over. A viewer request to a distribution carries NO AWS credential — nothing
//!   in CloudFront's model binds it, and CloudFront verifies no viewer signature of its
//!   own — so that operation declares no auth scheme and the router serves it unsigned.
//!   (A credential a viewer does present is not relayed to the origin either; [`edge`]'s
//!   `HEADERS_NEVER_FORWARDED` says why.) The claim
//!   is what narrows it: only a request whose host (or reserved data-plane path) names a
//!   distribution of this account is served that way, and every other request keeps going
//!   through the modelled routing table and its signature check.
//!
//! ## This service's other edited seam: the audit leg
//!
//! Every control-plane call this front door serves is a management event of the account,
//! so the routing table the envelope above dispatches from is the AUDITED one
//! ([`management_events`]): each served call is recorded in the account's event history
//! where CloudTrail's `LookupEvents` reads it, and — while a trail of the account is
//! logging — each MUTATING call is published onto the default EventBridge bus as `AWS API
//! Call via CloudTrail`. The EDGE plane is deliberately outside it: a viewer request is not
//! an API call of this account, so it is no management event.

/// Whether IAM lets a request perform the action it asks for on the resource it addressed
/// — the step an operation takes before it reads or writes anything.
pub mod authorization;
pub mod edge;
/// What the edge REPORTS about the viewer traffic it served: the per-distribution CloudWatch
/// series every answer it serves is counted into.
pub mod edge_metrics;
pub mod envelope;
/// This front door's AUDIT leg: every call it serves is recorded in the account's
/// management-event history, and fanned out onto the default event bus while a trail is
/// logging.
pub mod management_events;
pub mod origin_access;
/// The WAFv2 web ACL a distribution's configuration puts in front of it, and what the edge
/// answers a viewer request that web ACL does not let through.
pub mod web_acl;

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refusal, Refused,
    Service,
};
use std::sync::Arc;

/// The edge data plane as an [`ErasedOperation`], so the router hands it the
/// [`OperationContext`] carrying the in-process transport an origin fetch goes over.
///
/// It holds the SAME shared engine the generated entry threads into every operation, so
/// the edge reads the one store the control-plane operations write.
struct EdgeDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for EdgeDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            if edge::claims_request(&*self.engine, req) {
                return edge::serve(&*self.engine, req, ctx).await;
            }
            // Unreachable: `override_operation` names this only for a request the edge
            // claims.
            svc.definition().routing_error_response(req)
        })
    }

    /// NO auth scheme, which is what the router reads as "served unsigned".
    ///
    /// A viewer request to a distribution is not a modelled operation and carries no AWS
    /// credential: CloudFront's viewer side is plain HTTP(S), and CloudFront verifies no
    /// viewer signature of its own. This emulator goes one step further than CloudFront
    /// and does not relay a viewer's `Authorization` to the origin either (see
    /// `edge::HEADERS_NEVER_FORWARDED`): the origin is named by the distribution's stored
    /// configuration, so a credential the viewer presents would travel to a destination
    /// that viewer did not choose. The consequence is that an origin which authenticates
    /// the viewer itself sees no credential, and answers as it answers an anonymous
    /// request. The modelled operations of this service are untouched by this: they keep
    /// the schemes the model declares, because the routing table resolves them and the
    /// router reads the schemes off the modelled operation first.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

/// The `com-amazonaws-cloudfront-cloudfront2020-05-31` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: envelope::CloudFrontDefinition<Eng>,
    /// The edge data plane, as an operation, so the router hands it an
    /// [`OperationContext`] — the in-process transport an origin fetch goes over.
    edge: EdgeDataPlane<Eng>,
    /// Finishes the error envelope of every response this service answers, and restates
    /// a boundary refusal in this service's modelled vocabulary. Held here because
    /// [`Service::override_operation`] hands back a borrow of it.
    envelope: envelope::ErrorEnvelope,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: envelope::CloudFrontDefinition::new(Arc::clone(&engine)),
            edge: EdgeDataPlane {
                engine: Arc::clone(&engine),
            },
            envelope: envelope::ErrorEnvelope,
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
        // name — the control plane, by the framework's own rule.
        let control_plane = match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        };
        // Beside it, a viewer request addressed to the edge domain of a distribution this
        // service HOLDS: no route key names that host, so without this the emulator
        // answers `404 UnknownService` for the hostname a distribution publishes.
        control_plane || edge::claims_parts(&*self.edge.engine, parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request the model's routing table resolves is served through the
        // envelope seam, which runs that same table entry and then finishes the
        // response's error envelope (see the module docs and `envelope`). A request the
        // table does NOT resolve is left alone, so naming no modelled operation is still
        // refused in front of the service — in the entry's own envelope, which
        // `envelope::CloudFrontDefinition` finishes the same way.
        // A request whose HOST is a distribution's own edge domain is viewer traffic to
        // that distribution whatever its path spells — the host is what says which plane
        // a request belongs to — so it is served by the data plane even when its path
        // happens to match a modelled control-plane route.
        if edge::claims_request_by_host(&*self.edge.engine, req) {
            return Some(&self.edge as &dyn ErasedOperation);
        }
        if self.entry.routes(req) {
            return Some(&self.envelope as &dyn ErasedOperation);
        }
        // On the control plane's own host, or on the emulator's endpoint, the modelled
        // route wins and the reserved data-plane path (or a leading distribution id) is
        // read only for a path no modelled operation claims.
        edge::claims_request(&*self.edge.engine, req).then_some(&self.edge as &dyn ErasedOperation)
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
