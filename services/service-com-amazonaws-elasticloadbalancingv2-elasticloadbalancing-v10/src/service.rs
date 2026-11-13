// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10` service: how it claims requests and how it words the
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
//! Four of those seams are taken here, for four reasons.
//!
//! The first: this model's `endpointPrefix` and SigV4 signing name are the ones the Classic
//! Elastic Load Balancing model publishes too, so the claim cannot come from the model
//! alone. [`ROUTE_KEY`] carries that rule; the other half of it — how a request that
//! arrives on the SHARED key is served out of this model — belongs to the service that
//! claims that key, in
//! `services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs`.
//!
//! The second: an `awsQuery` error envelope carries a `<RequestId>` beside its `<Error>`,
//! which no model declares and the framework's error renderers do not write. So this
//! service's definition and its routable operations are served behind
//! `service_common::query_error_envelope`'s two wrappers, which add that one element to
//! every error response — modelled, refused or unroutable — and leave the status, the
//! code, the message and the whole success path with the framework renderers that own
//! them.
//!
//! The third: a load balancer this service creates publishes the name a client sends
//! traffic to, and that name is NOT this API's endpoint — so the front door could resolve
//! no target service from a request addressed to it and answered `404 UnknownService` for
//! the one address this service told the caller to use. [`can_handle`](Service::can_handle)
//! also claims a request addressed to a load balancer this service HOLDS, and
//! [`override_operation`](Service::override_operation) hands back the plane that answers it
//! out of that load balancer's listener configuration; [`data_plane`] is what resolves and
//! serves it, and [`lambda_target`] is the half of it that INVOKES a `lambda` target group's
//! function, while [`invalid_header_fields`] is the header fields that plane REMOVES from a
//! request before anything behind the load balancer sees them. A
//! request that plane serves is decided first by the WAFv2 web ACL `AssociateWebACL` put
//! in front of the load balancer ([`web_acl`]), which is what refuses it at the edge.
//!
//! The fourth: this API answers every request it refuses before a handler ran under ONE
//! error code, which the framework's two pre-dispatch layers do not agree on, so
//! [`override_refusal`](Service::override_refusal) states it.
//!
//! A fifth thing this file does is not a seam but a property of serving a call at all:
//! every control-plane call of an account is a CloudTrail management event, so this
//! service's definition routes through its operations wrapped in the shared
//! management-event recorder ([`management_events`]) — one code path for all of them,
//! reached whether the request arrives under this model's own routing key or is handed over
//! by the shared front door, rather than a recording written into each handler.

//! One fact about this service's wire identity is read from OUTSIDE this crate: the API
//! version an awsQuery envelope must name to be served out of this model on the endpoint
//! it SHARES with the Classic Elastic Load Balancing model. [`shared_endpoint`] publishes
//! it, so the front door that owns that shared name — and this crate's own suites — read
//! it rather than restating the literal.

pub mod shared_endpoint;

pub mod data_plane;

pub mod lambda_target;

pub mod invalid_header_fields;

pub mod target_group_selection;

pub mod web_acl;

pub mod listener_socket;

pub mod management_events;

use service_common::audited_operation::AuditedDefinition;
use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdEnvelopeOperation,
};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The routing key that names THIS service, as opposed to the endpoint prefix and SigV4
/// signing name it SHARES with the Classic Elastic Load Balancing model.
///
/// Elastic Load Balancing publishes ONE endpoint for two APIs: this model
/// (`sdkId: Elastic Load Balancing v2`, version `2015-12-01`) and the Classic model
/// (`sdkId: Elastic Load Balancing`, version `2012-06-01`,
/// `services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7`) both
/// declare `endpointPrefix` / `arnNamespace` / `aws.auth#sigv4` name
/// `elasticloadbalancing` (`smithy_query(query="service_info")` on either model). With
/// the generated signing-name claim on both, every Elastic Load Balancing request was
/// claimed twice and the router answered it `500 AmbiguousService` — a code no AWS model
/// carries, in a JSON body an awsQuery client cannot parse — leaving BOTH APIs
/// unreachable.
///
/// The shared name resolves to the ONE service whose own identity is that name: the
/// Classic model. That is not a lost operation for this one: an awsQuery envelope names
/// the API version it is for, so the Classic front door serves a `Version=2015-12-01`
/// request from THIS model, over the one shared engine (see that crate's
/// `src/service/api_version.rs`, which holds this model's own `EmulatedService` and hands
/// the request to the operation this model's routing table names) — which is how AWS
/// resolves the two APIs on their shared endpoint too. So an AWS SDK ELBv2 client, which
/// signs `elasticloadbalancing`, reaches these operations exactly as it does against AWS.
///
/// This key is what addresses this model DIRECTLY, without the shared front door: the
/// prefix derived from its own `sdkId`, as a credential scope or a host label. The
/// routing table is `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`.
const ROUTE_KEY: &str = "elasticloadbalancingv2";

/// The `com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, behind the AUDIT wrapper and the `awsQuery` error-envelope
    /// wrapper: every call this service serves is recorded as a management event (see
    /// [`management_events`]), and every error it answers carries the `<RequestId>` that
    /// envelope has (see `service_common::query_error_envelope`), whichever layer rendered
    /// it.
    ///
    /// The audit wrapper sits INSIDE, so the routing table every layer resolves an
    /// operation from — the router itself and the envelope wrapper below — is the audited
    /// one.
    entry: RequestIdEnvelopeDefinition<AuditedDefinition<crate::ServiceDefinition<Eng>>>,
    /// The envelope wrapper every routable request is served through. Held in a field,
    /// built once, because `override_operation` hands back a reference borrowed from
    /// `&self`.
    envelope: RequestIdEnvelopeOperation,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because a LOAD BALANCER's own traffic ([`data_plane`]) is claimed by
    /// `can_handle`, which is not an operation with a `self.engine` of its own: it must read
    /// the one store the control-plane operations write, not a second copy.
    engine: Arc<Eng>,
    /// The plane that SERVES a load balancer's own traffic, over that same shared engine.
    ///
    /// Held in a field, built once, because `override_operation` hands back a reference
    /// borrowed from `&self`.
    data_plane: data_plane::DataPlane<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::new(AuditedDefinition::new(
                crate::ServiceDefinition::new(Arc::clone(&engine)),
                // Every modelled operation, each constructed from this same shared engine
                // and served behind the audit recorder.
                management_events::audited_table(&engine),
            )),
            envelope: RequestIdEnvelopeOperation,
            data_plane: data_plane::DataPlane::new(Arc::clone(&engine)),
            engine,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        //
        // It is reached through the `awsQuery` envelope wrapper, which leaves identity,
        // protocol and the routing table exactly as the entry carries them and adds the
        // `<RequestId>` element to the framework-level error envelopes it renders — a
        // routing failure, a transport failure and a pre-dispatch refusal — so they carry
        // what a modelled error served through [`RequestIdEnvelopeOperation`] carries.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // The model does not express this service's identity: its signing name is
        // `elasticloadbalancing`, the endpoint prefix it shares with the Classic Elastic
        // Load Balancing model, so matching on the signing name alone claims that
        // model's traffic too and the router refuses both with `500 AmbiguousService`.
        // Claim only a request whose routing key names THIS service (see [`ROUTE_KEY`]);
        // the shared `elasticloadbalancing` name belongs to the Classic front door, which
        // serves this model's operations for an envelope naming this model's API version.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == ROUTE_KEY => true,
            // And a LOAD BALANCER's own traffic: a request addressed to a load balancer
            // this service holds a record for ([`data_plane::claims_parts`]). Such a
            // request names no modelled operation and carries no credential scope naming
            // this service — its host is the load balancer's own published `DNSName`, whose
            // first label names no service — so the front door could resolve no target
            // service for it and answered `404 UnknownService`, for the one address this
            // service told the caller to send traffic to.
            //
            // The claim is narrowed to a load balancer this service HOLDS, read from the
            // one shared store, rather than to a host or a path shape alone, so one request
            // keeps one claimant.
            _ => data_plane::claims_parts(&*self.engine, parts),
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
        // No ROUTING of a modelled operation is overridden: a request that arrives on
        // [`ROUTE_KEY`] is already addressed at this model, and the version half of the
        // shared endpoint's `(Action, Version)` rule is applied by the front door that owns
        // the shared key (the Classic crate's `src/service/api_version.rs`). What IS served
        // from here is the one request of this service that names no modelled operation at
        // all: a load balancer's own traffic.
        //
        // Every request the generated routing table can serve is served through the
        // envelope wrapper, which delegates to that same table entry and then adds the
        // `<RequestId>` the awsQuery error envelope carries. The routing stays the
        // model's: an `Action` naming no modelled operation is declined here, so the
        // caller of this seam answers it as the unroutable request it is — the router
        // through the wrapped definition above, and the shared front door (which asks this
        // seam first) through its own action/version refusal.
        // A LOAD BALANCER's own traffic is served by the data plane, which is the one
        // request this service answers that names no modelled operation by construction (no
        // modelled operation is bound at a load balancer's request path). It gets first
        // refusal, so the claim `can_handle` made ([`data_plane::claims_parts`]) and the
        // serving are decided by the one rule.
        if self.data_plane.claims(req) {
            return Some(&self.data_plane as _);
        }
        self.entry.routes(req).then_some(&self.envelope as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) before any handler ran.
        //
        // Elastic Load Balancing answers EVERY such refusal under ONE code,
        // `ValidationError` (`crate::scaffolded::VALIDATION_ERROR`) — the code its
        // handlers answer a value they cannot accept with, and the code the framework
        // already picks for a value outside a modelled bound on the query protocols. The
        // framework parts the two layers it finds a refusal in: a member it could not
        // READ at all (an absent required one travels as a codec failure) renders as
        // `MalformedInput`, a code this API does not answer and no client branches on, so
        // one logical failure answered under two codes. Naming the one code here is what
        // closes that.
        //
        // The MESSAGE is the framework's own envelope over the same violations, so the
        // sentence a caller reads is unchanged and the two layers cannot drift: only the
        // code is restated. `None` when there is nothing to state, and for an
        // authentication refusal, which keeps the framework's 403 and its wording.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: crate::scaffolded::VALIDATION_ERROR.to_string(),
                message: aws_smithy_schema_server::envelope(violations)?,
            }),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. A load balancer's own traffic is served through
        // `override_operation` instead ([`data_plane::DataPlane`]), which is what hands that
        // plane the in-process transport a `forward` to a `lambda` target group invokes the
        // function over.
        //
        // Anything else keeps the protocol-correct default for this service's envelope
        // (query -> 400 InvalidAction text/xml) by returning `None`.
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
