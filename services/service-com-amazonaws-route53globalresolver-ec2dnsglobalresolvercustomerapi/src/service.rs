// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi` service: how it claims requests and how it words the
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
//!   correctly, or answer one before the modelled walk runs. This service uses it for
//!   the authorization it takes on a caller-supplied resource ARN ahead of that walk
//!   (`resource_arn_authorization`), and for the one listing whose input the framework's
//!   HTTP binding cannot read (`core_network_endpoint_listing`); every other request
//!   resolves through the generated routing table.
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
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The authorization this service takes on a caller-supplied resource ARN ahead of the
/// modelled constraints on the member that carried it: a value that names no resource is an
/// authorization failure, not a validation one.
mod resource_arn_authorization;

/// `ListCoreNetworkEndpoints`'s own query-string binding, which the framework's
/// `@httpQueryParams` binding cannot do — see that module.
mod core_network_endpoint_listing;

/// The error code this service answers a request refused for a modelled input
/// constraint with: the `ValidationException` (`@httpError 400`) every operation of
/// this service declares, which is also the code the framework's own default picks
/// there. Restated here because a reworded refusal replaces the code and the message
/// together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `violation` with its `@pattern` requirement quoted the way this service quotes one:
/// the expression WITHOUT the whole-value anchors the model writes around it
/// (`service_common::validation::unanchored`, the shared home for that rendering).
/// Every other violation is left exactly as the framework's walk reported it.
///
/// Only the RENDERING drops the anchors: the check the walk ran used the model's
/// pattern as written, so a value that merely contains a match is still refused.
fn requoting_pattern(violation: &Violation) -> Violation {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return violation.clone();
    };
    Violation {
        kind: ViolationKind::Pattern(::service_common::validation::unanchored(pattern)),
        ..violation.clone()
    }
}

/// State a request the framework refused before any handler ran the way this service
/// states it, or `None` to keep the framework's own wording.
///
/// The envelope (`N validation error(s) detected: `), the clause
/// `Value '<value>' at '<member path>' failed to satisfy constraint: <requirement>`, the
/// member paths and every requirement stay the framework's
/// (`aws_smithy_schema_server::envelope`), so this service cannot drift from them; the
/// one thing restated is how a `@pattern` requirement quotes its expression.
fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let restated: Vec<Violation> = violations.iter().map(requoting_pattern).collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: ::aws_smithy_schema_server::envelope(&restated)?,
    })
}

/// The `com-amazonaws-route53globalresolver-ec2dnsglobalresolvercustomerapi` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The 403 answer for a call that names its resource by a value that is not an ARN. Held
    /// here so [`Service::override_operation`] can hand out a reference to it; it carries no
    /// state, because the denial is built from the request alone.
    deny_unnamed_resource: resource_arn_authorization::DenyUnnamedResource,
    /// `ListCoreNetworkEndpoints` served off its own query-string binding, for the reason
    /// [`core_network_endpoint_listing`] states. Held here for the same reason as above:
    /// [`Service::override_operation`] hands back a borrow.
    list_core_network_endpoints:
        core_network_endpoint_listing::ListCoreNetworkEndpointsFromQuery<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            deny_unnamed_resource: resource_arn_authorization::DenyUnnamedResource,
            list_core_network_endpoints:
                core_network_endpoint_listing::ListCoreNetworkEndpointsFromQuery::new(engine),
        }
    }

    /// Does `req` address a resource this service cannot authorize the caller against — a
    /// `resourceArn` that is not an ARN at all?
    ///
    /// The read itself is [`resource_arn_authorization::unauthorized_resource`], the one the
    /// refusal is built from too, so the gate and the message it answers with cannot disagree
    /// about which request is refused.
    fn names_no_resource_to_authorize(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        let protocol = self.entry.wire_protocol(req);
        resource_arn_authorization::unauthorized_resource(protocol.protocol(), req).is_some()
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
        // Authorization comes before request validation: an operation whose only addressed
        // resource is the ARN the caller supplies is authorized against that value as
        // supplied, so one that is not an ARN names no resource a policy statement could
        // have granted and is refused `AccessDeniedException` (403) here rather than falling
        // to the modelled `@pattern` rejection (400) the framework's pre-handler walk would
        // answer with. See [`resource_arn_authorization`]. Every other request resolves
        // through the generated routing table — bar the one listing whose input the
        // framework cannot bind at all, which is served off its own binding
        // ([`core_network_endpoint_listing`]).
        if self.names_no_resource_to_authorize(req) {
            return Some(&self.deny_unnamed_resource);
        }
        let key = self.entry.protocol().routing_key(req).ok()?;
        (key
            == core_network_endpoint_listing::ListCoreNetworkEndpointsFromQuery::<Eng>::routing_key())
            .then_some(&self.list_core_network_endpoints as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. A refusal for a modelled input constraint is stated
        // with the `@pattern` requirement quoted as this service quotes one — the
        // expression without the whole-value anchors the model writes around it —
        // and with the framework's envelope, clauses and member paths otherwise
        // untouched. The code travels with the message: it stays the modelled
        // `ValidationException`, which is what the framework answers too. An
        // authentication refusal is not an input-constraint failure, so it keeps
        // the framework's 403 code and wording.
        stated_refusal(refusal)
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
