// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-paymentcryptographydata-paymentcryptographydataplane` service: how it claims requests and how it words the
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
//!   correctly. THIS SERVICE USES IT: every modelled operation is served behind the
//!   modelled-refusal wrapper in [`validation`], which answers a request refused BEFORE
//!   the handler runs — a `@required` member absent, a value outside a declared bound —
//!   in this service's own modelled `ValidationException`, with the `fieldList` the
//!   model declares, and hands every other request to the generated operation unchanged.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS SERVICE REWORDS IT, for whatever still renders there:
//!   the same envelope and clause [`validation`] states, which that hook can reword but
//!   not reshape.
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
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

pub mod validation;

/// The `com-amazonaws-paymentcryptographydata-paymentcryptographydataplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// One modelled-refusal wrapper per modelled operation, keyed by the routing key the
    /// model names that operation by ([`smithy_server_runtime::Operation::NAME`], the same
    /// key the generated routing table uses). Built once here, never per request; each
    /// entry is zero-sized, so this is a table of NAMES rather than a second set of
    /// operations — the operation that serves a request is still the one the generated
    /// table built from the shared engine.
    refusals: ::std::collections::HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            refusals: validation::modelled_validation_table::<Eng>(),
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
        // This service signs as `payment-cryptography` — a signing name it SHARES with
        // the Payment Cryptography CONTROL plane, which declares the same `sigv4Name` and
        // is a wire service of its own (`controlplane.payment-cryptography`,
        // `awsJson1_0`) against this one's `dataplane.payment-cryptography`, `restJson1`.
        // The router resolves the routing key from the SigV4 credential scope first, so
        // both services see the key `payment-cryptography`: claiming on the signing name
        // ALONE makes each claim the other's traffic, and the router — which refuses
        // rather than guesses — answers `AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing name.
        // This model is `restJson1`: its surface is the URI path space its operations bind
        // (`/keys/…`, `/pindata/…`, `/mac/…`, and the rest), which is what
        // `claims_request` matches against the generated `ROUTE_PATH_SEGMENTS` when a
        // service declares no RPC protocol. The control plane's `awsJson1_0` traffic posts
        // to the root and names its own service shape, so it is claimed there alone.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve every modelled operation through its modelled-refusal wrapper, so a
        // request this service refuses before the handler runs is stated in the service's
        // OWN modelled `ValidationException` — with the `fieldList` the model declares —
        // instead of the framework's two-field refusal body. The wrapper hands every
        // request it does not refuse to the generated operation in the routing table,
        // which remains the only thing that serves one.
        //
        // Keyed by the protocol's own routing key, so a request the model names no
        // operation for resolves to nothing here and keeps the framework's routing
        // failure.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // This hook can reword a refusal but not RESHAPE it: `Refused` carries a code and
        // a message, so nothing reached through here can carry the `fieldList` the model
        // declares on `ValidationException`. Every boundary refusal of this service is
        // therefore made where the modelled shape can be returned — in the wrapper in
        // [`validation`] — and this stays as the wording of last resort for a refusal
        // that still renders here, so that the sentence a caller reads is the same one
        // either way.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: validation::VALIDATION_EXCEPTION.to_string(),
                message: validation::boundary_exception(violations)?.message,
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
