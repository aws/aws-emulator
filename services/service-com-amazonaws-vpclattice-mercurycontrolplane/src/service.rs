// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-vpclattice-mercurycontrolplane` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers a request refused for a modelled input
/// constraint with: the `ValidationException` (`@httpError 400`) every operation of this
/// service declares, which is also the code the framework's own default picks there.
/// Restated here because a reworded refusal replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// `violation` with its `@pattern` requirement quoted the way this service quotes one:
/// the expression without the whole-value anchors the model writes around it
/// (`service_common::validation::unanchored`, the shared home for that rendering, which
/// `crate::state::reported_pattern` renders a handler-checked pattern through too).
/// Every other violation is left exactly as the framework's walk reported it.
///
/// Only the RENDERING drops the anchors: the check the walk ran used the model's pattern
/// as written, so a value that merely contains a match is still refused.
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
///
/// `service_common::aws_validation_refusal` is not used for the whole refusal because it
/// also drops the value half of a COLLECTION's `@length` clause, which this service
/// states with the framework's own summary of the collection; the `@pattern` rendering is
/// taken from that crate's shared home for it instead.
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

/// The `com-amazonaws-vpclattice-mercurycontrolplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. A refusal for a modelled input constraint is stated
        // with the `@pattern` requirement quoted as this service quotes one — the
        // expression without the whole-value anchors the model writes around it —
        // and with the framework's envelope, clauses and member paths otherwise
        // untouched. The code travels with the message: it stays the modelled
        // `ValidationException`, which is what the framework answers too.
        // An authentication refusal is not an input-constraint failure, so it
        // keeps the framework's 403 code and wording.
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
