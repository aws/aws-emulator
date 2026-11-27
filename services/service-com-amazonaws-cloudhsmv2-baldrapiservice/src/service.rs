// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudhsmv2-baldrapiservice` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: this service's
//!   SigV4 signing name is shared with CloudHSM Classic, which the model cannot
//!   express, so the discriminator is the service the request's own RPC wire form
//!   names — in either of the two protocols this model declares.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE: every MODELLED operation is served under the key the
//!   generated table itself resolves, wrapped so that the error bodies it answers with
//!   are the ones this service answers with ([`stated_error_bodies`]).
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

mod cbor_error_bodies;
mod error_message_member;
mod stated_error_bodies;
mod validation;

/// The routing key an UNSIGNED request of this service resolves to: the framework reads
/// it out of the `X-Amz-Target` header this service's awsJson1_1 operations carry, taking
/// the segment before the `.`, stripping a `_<version>` suffix and lower-casing it, so
/// `BaldrApiService.<Operation>` normalises to this key — while CloudHSM Classic's own
/// `CloudHsmFrontendService.<Operation>` normalises to `cloudhsmfrontendservice`.
///
/// A SIGNED request resolves to the signing name instead, which is shared with CloudHSM
/// Classic, so there the service the request's own RPC wire form NAMES is the
/// discriminator ([`Service::can_handle`]).
const TARGET_SERVICE_KEY: &str = "baldrapiservice";

/// The `com-amazonaws-cloudhsmv2-baldrapiservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped so that the
    /// error bodies it answers with are the ones this service answers with
    /// ([`stated_error_bodies`]). Keyed by the routing key the protocol resolves — the
    /// key the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    stated_error_bodies: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            stated_error_bodies: stated_error_bodies::stated_error_bodies(&engine),
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
        // CloudHSM V2 and CloudHSM Classic are two services behind ONE SigV4 signing
        // name (`cloudhsm`), which the model cannot express, so the default
        // signing-name match makes BOTH claim every CloudHSM-signed request and the
        // router rejects all of it rather than serving it from an arbitrary one. What
        // tells the two apart is the service the request's own RPC wire form NAMES, and
        // this model declares TWO such forms: `X-Amz-Target: BaldrApiService.<Operation>`
        // on awsJson1_1, and the `{Service}` label of
        // `POST /service/BaldrApiService/operation/<Operation>` on rpcv2Cbor
        // (`smithy_query(query="service_info")`). `claims_request` is the one rule that
        // reads either form against this service's own shape name, so this service claims
        // exactly its own traffic in both protocols — and the Classic crate, reading the
        // same rule against ITS shape name, declines all of it.
        if ::service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            Some(self.entry.identity().shape_id()),
        ) {
            return true;
        }
        // An UNSIGNED request carries no credential scope, so the resolver's next
        // signal is the `X-Amz-Target` prefix itself: a key that IS this service's
        // target prefix names this service by the same discriminator.
        smithy_server_runtime::resolve_route_key(parts).as_deref() == Some(TARGET_SERVICE_KEY)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped so that every error body
        // reports its text in the member this model declares
        // ([`error_message_member`]). A key the model does not name resolves to `None`
        // here exactly as it does in the generated table, so the router's routing-error
        // path is unchanged.
        //
        // The key is read in the protocol THIS REQUEST is served in, which is the one the
        // generated table is keyed by for it: this model declares two protocols, and each
        // names the operation in its own place (`X-Amz-Target` on awsJson1_1, the path on
        // rpcv2Cbor). Reading it in the first DECLARED protocol instead would resolve no
        // key for a request in the other one, so every such request would bypass this
        // layer and answer its failures in the framework's wording.
        let wire = self.entry.wire_protocol(req);
        let key = wire.protocol().routing_key(req).ok()?;
        self.stated_error_bodies.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // CloudHSM V2 models NO `ValidationException` — the framework's default code for
        // this class — and declares one error for it instead:
        // `CloudHsmInvalidRequestException`, documented as "The request was rejected
        // because it is not a valid request". That sentence describes exactly this
        // refusal class, so every operation of this service answers a request that
        // violates its modelled input shape with the code its own model declares, the way
        // a handler does for the rules the model cannot express. A client matching the
        // operation's modelled errors can then classify the refusal at all.
        //
        // The framework's MESSAGE is kept: it is already the envelope AWS uses —
        // `<N> validation error(s) detected: ` over one clause per violation, naming the
        // member as a path spells it, quoting the value the request carried and stating
        // the requirement it failed, with a `@pattern` quoted as the model writes it.
        // The one half the walk cannot state is the COLLECTION a clause about a
        // collection member is about, which the per-operation layer in
        // [`stated_error_bodies`] states before this seam is reached.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: stated_error_bodies::CLOUD_HSM_INVALID_REQUEST.to_string(),
                message: ::aws_smithy_schema_server::envelope(violations)?,
            }),
            // Authentication is the emulator's, not this service's: its model declares no
            // shape for a refused caller, so the framework's 403 stands.
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
