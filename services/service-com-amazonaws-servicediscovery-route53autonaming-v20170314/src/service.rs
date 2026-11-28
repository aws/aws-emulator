// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-servicediscovery-route53autonaming-v20170314` service: how it claims requests and how it words the
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
//!   correctly. THIS SERVICE answers it with the same operation the generated table
//!   resolves, wrapped so that the error bodies it answers with are the ones this
//!   service answers with: see [`submitted_collections`] and
//!   [`error_message_member`].
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

mod data_endpoint;
mod enum_value_sets;
mod error_message_member;
mod modelled_patterns;
mod submitted_collections;
mod submitted_map;

use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-servicediscovery-route53autonaming-v20170314` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped so that the
    /// error bodies it answers with are the ones this service answers with
    /// ([`submitted_collections::stated_error_bodies`]). Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
    stated_error_bodies: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            stated_error_bodies: submitted_collections::stated_error_bodies(&engine),
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped so that a constraint
        // refusal quotes the collection the caller submitted and every error body reports
        // its text in the member this model declares. A key the model does not name
        // resolves to `None` here exactly as it does in the generated table, so the
        // router's routing-error path is unchanged.
        //
        // The key is read with the codec THIS REQUEST is served in, which the framework's
        // own dispatch selects per request: this model declares two protocols, and each
        // names an operation somewhere else (one in a header, the other in the request
        // path), so a key read with the service's first-declared codec alone would resolve
        // for one of them and fail for the other — leaving every request in that protocol
        // to the unwrapped table, answered in error bodies this service does not answer in.
        let key = self
            .entry
            .wire_protocol(req)
            .protocol()
            .routing_key(req)
            .ok()?;
        self.stated_error_bodies.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Cloud Map models NO `ValidationException` — the framework's default code
        // for this class — and declares exactly one error for it instead:
        // `InvalidInput`, documented as "One or more specified values aren't valid.
        // For example, a required value might be missing, a numeric value might be
        // outside the allowed range, or a string value might exceed length
        // constraints." That sentence describes this refusal class member for member,
        // so every operation of this service answers a boundary refusal with the code
        // its own model declares, the way a handler does for the rules the model
        // cannot express. The framework's message is kept: it already names the member
        // and the constraint it failed, in the envelope AWS itself uses. What this service
        // states of its own is the ORDER an enumerated type's admissible set is listed in
        // ([`enum_value_sets`]), a fact of the front door holding those values in a table
        // rather than of the model's declaration.
        //
        // This is the wording of the service's MAIN endpoint. The two operations the model
        // hosts on a prefixed endpoint of their own are fronted by AWS's generic
        // request-validation layer, which states the same sentence under the code IT names
        // ([`data_endpoint`]); that layer answers before this seam is reached, so the two
        // cannot both word one refusal.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidInput".to_string(),
                message: enum_value_sets::stated_message(violations)
                    .unwrap_or_else(|| "One or more specified values aren't valid.".to_string()),
            }),
            // Authentication is the emulator's, not this service's: its model declares
            // no shape for a refused caller, so the framework's 403 stands.
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
